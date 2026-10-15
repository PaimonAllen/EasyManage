//! Fixed-name, metadata-only AI Agent tool inventory. Not inference runtimes,
//! model providers or programming environments. Never execute a discovered binary
//! or read model settings, command lines, environment variables, or credentials.
use easymanage_protocol::{AiToolProcess, AiToolStatus, CollectionIssue, ServiceStatus};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HOME_LIMIT: usize = 64;
const DIRECTORY_LIMIT: usize = 1024;
const ENTRY_LIMIT: usize = 128;
const RELEASE_LIMIT: usize = 16;
const PATH_LIMIT: usize = 32;
const SCAN_BUDGET: Duration = Duration::from_secs(2);

const KNOWN_TOOLS: &[(&str, &str)] = &[
    ("codex", "assistant"),
    ("claude", "assistant"),
    ("gemini", "assistant"),
    ("opencode", "assistant"),
    ("aider", "assistant"),
];

pub fn recognize(process_name: &str) -> Option<&'static str> {
    KNOWN_TOOLS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(process_name))
        .map(|(name, _)| *name)
}

fn safe_home(value: &str) -> bool {
    value == "/root"
        || value.strip_prefix("/home/").is_some_and(|username| {
            !username.is_empty()
                && !matches!(username, "." | "..")
                && username
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
}

pub fn executable_paths(name: &str, agent_home: Option<&str>) -> Vec<String> {
    let mut directories = vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ];
    if let Some(home) = agent_home.filter(|home| safe_home(home)) {
        for relative in [".local/bin", ".npm-global/bin", ".cargo/bin"] {
            directories.push(PathBuf::from(home).join(relative));
        }
    }
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .filter(|path| {
            fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

pub fn inventory(
    account_homes: &[String],
    observed: &HashMap<&'static str, Vec<AiToolProcess>>,
    running_services: &[ServiceStatus],
    issues: &mut Vec<CollectionIssue>,
) -> Vec<AiToolStatus> {
    let mut homes: Vec<_> = account_homes
        .iter()
        .filter(|home| safe_home(home))
        .map(PathBuf::from)
        .collect();
    homes.sort();
    homes.dedup();
    let omitted_homes = homes.len().saturating_sub(HOME_LIMIT);
    homes.truncate(HOME_LIMIT);
    let mut scan = scan_paths(
        &homes,
        &system_directories(),
        &[
            PathBuf::from("/opt/conda/envs"),
            PathBuf::from("/opt/miniconda3/envs"),
            PathBuf::from("/opt/anaconda3/envs"),
        ],
    );
    if omitted_homes > 0 {
        scan.notes.push(format!(
            "账户目录超过 {HOME_LIMIT} 个上限，另有 {omitted_homes} 个未扫描"
        ));
    }
    let invalid = account_homes.iter().filter(|home| !safe_home(home)).count();
    if invalid > 0 {
        scan.notes.push(format!(
            "{invalid} 个账户 home 不在安全的 /home/<账户> 或 /root 范围，未扫描"
        ));
    }
    for message in &scan.notes {
        issues.push(CollectionIssue {
            component: "ai_tools".to_owned(),
            message: message.clone(),
        });
    }
    KNOWN_TOOLS
        .iter()
        .map(|(name, category)| {
            let executable_paths = scan.paths.remove(*name).unwrap_or_default();
            let service_units = running_services
                .iter()
                .filter(|service| {
                    service
                        .name
                        .strip_suffix(".service")
                        .and_then(|unit| unit.split('@').next())
                        .is_some_and(|unit| unit.eq_ignore_ascii_case(name))
                })
                .take(16)
                .map(|service| service.name.clone())
                .collect();
            AiToolStatus {
                name: (*name).to_owned(),
                category: (*category).to_owned(),
                executable_paths,
                processes: observed.get(name).cloned().unwrap_or_default(),
                service_units,
            }
        })
        .collect()
}

fn system_directories() -> Vec<PathBuf> {
    [
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/snap/bin",
        "/opt/conda/bin",
        "/opt/miniconda3/bin",
        "/opt/anaconda3/bin",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

#[derive(Default)]
struct ScanResult {
    paths: HashMap<&'static str, Vec<String>>,
    notes: Vec<String>,
}

struct Scanner {
    result: ScanResult,
    seen: HashSet<PathBuf>,
    started: Instant,
    denied: usize,
    failed: usize,
    capped: bool,
    timed_out: bool,
}

impl Scanner {
    fn new() -> Self {
        Self {
            result: ScanResult::default(),
            seen: HashSet::new(),
            started: Instant::now(),
            denied: 0,
            failed: 0,
            capped: false,
            timed_out: false,
        }
    }

    fn ready(&mut self) -> bool {
        if self.started.elapsed() >= SCAN_BUDGET {
            self.timed_out = true;
            return false;
        }
        if self.seen.len() >= DIRECTORY_LIMIT {
            self.capped = true;
            return false;
        }
        true
    }

    fn error(&mut self, error: &io::Error) {
        match error.kind() {
            io::ErrorKind::NotFound => {}
            io::ErrorKind::PermissionDenied => self.denied += 1,
            _ => self.failed += 1,
        }
    }

    fn visit(&mut self, directory: &Path) -> bool {
        if !self.ready() || !self.seen.insert(directory.to_path_buf()) {
            return false;
        }
        true
    }

    fn binaries(&mut self, directory: &Path) {
        if !self.visit(directory) {
            return;
        }
        match fs::metadata(directory) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return,
            Err(error) => {
                self.error(&error);
                return;
            }
        }
        for &(name, _) in KNOWN_TOOLS {
            if !self.ready() {
                break;
            }
            let path = directory.join(name);
            match fs::metadata(&path) {
                Ok(metadata)
                    if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 =>
                {
                    let paths = self.result.paths.entry(name).or_default();
                    if paths.len() < PATH_LIMIT {
                        paths.push(path.to_string_lossy().into_owned());
                    } else {
                        self.capped = true;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    let denied = error.kind() == io::ErrorKind::PermissionDenied;
                    self.error(&error);
                    if denied {
                        break;
                    }
                }
            }
        }
    }

    // Only one level under an explicit installation root, never project trees.
    fn releases(&mut self, root: &Path, suffix: &str) {
        if !self.visit(root) {
            return;
        }
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) => {
                self.error(&error);
                return;
            }
        };
        let mut directories = Vec::new();
        for (index, entry) in entries.enumerate() {
            if !self.ready() {
                break;
            }
            if index >= ENTRY_LIMIT {
                self.capped = true;
                break;
            }
            match entry {
                Ok(entry) => {
                    let name = entry.file_name();
                    if name.to_str().is_some_and(|name| {
                        !name.starts_with('.')
                            && name
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                    }) {
                        directories.push(entry.path());
                    }
                }
                Err(error) => self.error(&error),
            }
        }
        directories.sort();
        if directories.len() > RELEASE_LIMIT {
            self.capped = true;
        }
        for directory in directories.into_iter().take(RELEASE_LIMIT) {
            self.binaries(&directory.join(suffix));
        }
    }

    fn finish(mut self) -> ScanResult {
        if self.denied > 0 {
            self.result.notes.push(format!(
                "AI 安装目录有 {} 处权限不足；已保留可见结果，未发现不代表未安装",
                self.denied
            ));
        }
        if self.failed > 0 {
            self.result.notes.push(format!(
                "AI 安装目录有 {} 处读取失败；扫描结果可能不完整",
                self.failed
            ));
        }
        if self.capped {
            self.result.notes.push(format!(
            "AI 扫描达到数量上限（目录 {DIRECTORY_LIMIT}、单目录条目 {ENTRY_LIMIT}、版本/环境 {RELEASE_LIMIT}、每工具路径 {PATH_LIMIT}），结果可能不完整"));
        }
        if self.timed_out {
            self.result.notes.push(
            "AI 扫描达到 2 秒工作预算，已停止后续探测；文件系统单次读取不保证硬超时，结果可能不完整".to_owned());
        }
        self.result
    }
}

// Production callers validate account home paths first; injectable roots allow
// disposable fixtures to prove discovery without touching any real user files.
fn scan_paths(homes: &[PathBuf], systems: &[PathBuf], system_envs: &[PathBuf]) -> ScanResult {
    let mut scan = Scanner::new();
    for directory in systems {
        scan.binaries(directory);
    }
    for root in system_envs {
        scan.releases(root, "bin");
    }
    for home in homes {
        if !scan.ready() {
            break;
        }
        for relative in [
            ".local/bin",
            ".npm-global/bin",
            ".npm/bin",
            ".cargo/bin",
            ".bun/bin",
            ".opencode/bin",
            ".local/share/pnpm",
            "miniconda3/bin",
            "anaconda3/bin",
            "miniforge3/bin",
            "mambaforge/bin",
            ".venv/bin",
            "venv/bin",
        ] {
            scan.binaries(&home.join(relative));
        }
        for (relative, suffix) in [
            (".nvm/versions/node", "bin"),
            (".local/share/fnm/node-versions", "installation/bin"),
            (".local/share/mise/installs/node", "bin"),
            (".conda/envs", "bin"),
            ("miniconda3/envs", "bin"),
            ("anaconda3/envs", "bin"),
            ("miniforge3/envs", "bin"),
            ("mambaforge/envs", "bin"),
            (".local/share/pipx/venvs", "bin"),
            (".local/pipx/venvs", "bin"),
            (".local/share/uv/tools", "bin"),
        ] {
            scan.releases(&home.join(relative), suffix);
        }
    }
    scan.finish()
}

#[cfg(test)]
mod tests {
    use super::{
        DIRECTORY_LIMIT, PATH_LIMIT, SCAN_BUDGET, Scanner, inventory, recognize, safe_home,
        scan_paths,
    };
    use easymanage_protocol::{AiToolProcess, ServiceStatus};
    use std::collections::HashMap;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("easymanage-ai-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn executable(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // Deliberately not a runnable program: inventory must inspect metadata only.
            fs::write(&path, b"fixture, never execute").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn finds_other_users_and_common_managers_without_executing_files() {
        let fixture = Fixture::new();
        let expected = [
            ("alice/.npm-global/bin/codex", "codex"),
            ("bob/.local/bin/claude", "claude"),
            ("bob/.nvm/versions/node/v22.1.0/bin/gemini", "gemini"),
            (
                "bob/.local/share/fnm/node-versions/v20.0.0/installation/bin/codex",
                "codex",
            ),
            (
                "bob/.local/share/mise/installs/node/22.0.0/bin/codex",
                "codex",
            ),
            ("bob/.bun/bin/opencode", "opencode"),
            ("bob/miniconda3/envs/assistant/bin/aider", "aider"),
            ("bob/.conda/envs/assistant/bin/codex", "codex"),
            ("bob/.local/share/pipx/venvs/aider/bin/aider", "aider"),
            ("bob/.local/share/uv/tools/aider/bin/aider", "aider"),
            ("system-envs/assistant/bin/aider", "aider"),
        ];
        for (relative, _) in expected {
            fixture.executable(relative);
        }
        let project_tool = fixture.executable("bob/projects/private/.venv/bin/codex");
        fixture.executable("bob/.local/bin/ollama");
        let result = scan_paths(
            &[fixture.0.join("alice"), fixture.0.join("bob")],
            &[],
            &[fixture.0.join("system-envs")],
        );
        for (relative, tool) in expected {
            assert!(
                result.paths[tool]
                    .contains(&fixture.0.join(relative).to_string_lossy().into_owned())
            );
        }
        assert!(
            !result.paths["codex"].contains(&project_tool.to_string_lossy().into_owned()),
            "do not recurse into project trees"
        );
        assert!(
            !result.paths.contains_key("ollama"),
            "inference runtimes are outside scope"
        );
        assert!(result.notes.is_empty(), "{}", result.notes.join("; "));
    }

    #[test]
    fn follows_binary_symlinks_but_ignores_broken_links_and_nonexecutables() {
        let fixture = Fixture::new();
        let target = fixture.executable("package/cli.js");
        let bin = fixture.0.join("bin");
        fs::create_dir(&bin).unwrap();
        std::os::unix::fs::symlink(target, bin.join("codex")).unwrap();
        std::os::unix::fs::symlink("/missing-ai-fixture", bin.join("claude")).unwrap();
        fs::write(bin.join("gemini"), b"not executable").unwrap();
        fs::create_dir(bin.join("opencode")).unwrap();
        let result = scan_paths(&[], &[bin.clone(), bin.clone()], &[]);
        assert_eq!(
            result.paths["codex"],
            [bin.join("codex").to_string_lossy().into_owned()]
        );
        assert_eq!(result.paths.len(), 1);
    }

    #[test]
    fn environment_scan_is_bounded_and_preserves_visible_results() {
        let fixture = Fixture::new();
        for index in 0..20 {
            fixture.executable(&format!("user/.conda/envs/env{index:02}/bin/aider"));
        }
        let result = scan_paths(&[fixture.0.join("user")], &[], &[]);
        assert_eq!(result.paths["aider"].len(), 16);
        assert!(result.notes.iter().any(|note| note.contains("数量上限")));
    }

    #[test]
    fn unreadable_user_directory_is_not_silently_reported_as_uninstalled() {
        let fixture = Fixture::new();
        fixture.executable("locked/bin/codex");
        let locked = fixture.0.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let result = scan_paths(&[], &[locked.join("bin")], &[]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.paths.is_empty());
        assert!(
            result.notes.iter().any(|note| note.contains("权限不足")),
            "run tests as non-root in isolation"
        );
    }

    #[test]
    fn home_cap_and_unsafe_account_paths_are_visible_in_collection_notes() {
        let mut homes: Vec<_> = (0..65)
            .map(|index| format!("/home/ai-fixture-{index}"))
            .collect();
        homes.push("/home/..".to_owned());
        homes.push("/srv/private".to_owned());
        let mut issues = Vec::new();
        let tools = inventory(&homes, &HashMap::new(), &[], &mut issues);
        assert_eq!(tools.len(), super::KNOWN_TOOLS.len());
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("另有 1 个未扫描"))
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("2 个账户 home"))
        );
        assert!(issues.iter().all(|issue| issue.component == "ai_tools"));
    }

    #[test]
    fn permission_errors_and_time_directory_path_limits_are_explicit() {
        let mut denied = Scanner::new();
        denied.error(&std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let denied = denied.finish();
        assert!(denied.notes[0].contains("权限不足"));
        let mut timeout = Scanner::new();
        timeout.started -= SCAN_BUDGET;
        timeout.binaries(Path::new("/not-inspected"));
        assert!(timeout.finish().notes[0].contains("工作预算"));
        let mut capped = Scanner::new();
        for index in 0..DIRECTORY_LIMIT {
            capped
                .seen
                .insert(PathBuf::from(format!("/fixture/{index}")));
        }
        capped.binaries(Path::new("/not-inspected"));
        assert!(capped.finish().notes[0].contains("数量上限"));
        let fixture = Fixture::new();
        let bins: Vec<_> = (0..PATH_LIMIT + 1)
            .map(|index| {
                fixture
                    .executable(&format!("bin{index}/codex"))
                    .parent()
                    .unwrap()
                    .to_path_buf()
            })
            .collect();
        let result = scan_paths(&[], &bins, &[]);
        assert_eq!(result.paths["codex"].len(), PATH_LIMIT);
        assert!(result.notes[0].contains("数量上限"));
    }

    #[test]
    fn recognizes_only_explicit_ai_process_names() {
        assert_eq!(recognize("codex"), Some("codex"));
        assert_eq!(recognize("Claude"), Some("claude"));
        for generic in ["python", "node", "bash", "vllm-worker-other", ""] {
            assert_eq!(recognize(generic), None);
        }
        for runtime in [
            "ollama",
            "llama-server",
            "vllm",
            "litellm",
            "open-webui",
            "comfyui",
            "lms",
        ] {
            assert_eq!(recognize(runtime), None);
        }
    }

    #[test]
    fn limits_user_binary_search_to_expected_home_layout() {
        assert!(safe_home("/home/tzr"));
        assert!(safe_home("/root"));
        for unsafe_home in [
            "/tmp/other",
            "/home/../root",
            "/home/user/other",
            "/home/.",
            "/home/..",
            "",
        ] {
            assert!(!safe_home(unsafe_home));
        }
    }

    #[test]
    fn combines_only_matching_processes_and_services() {
        let mut processes = HashMap::new();
        processes.insert(
            "claude",
            vec![AiToolProcess {
                pid: 42,
                uid: 1000,
                username: "tester".to_owned(),
            }],
        );
        let services = [
            ServiceStatus {
                name: "codex@worker.service".to_owned(),
                state: "running".to_owned(),
            },
            ServiceStatus {
                name: "ollama.service".to_owned(),
                state: "running".to_owned(),
            },
        ];
        let tools = inventory(&[], &processes, &services, &mut Vec::new());
        let claude = tools.iter().find(|tool| tool.name == "claude").unwrap();
        let codex = tools.iter().find(|tool| tool.name == "codex").unwrap();
        assert_eq!(claude.processes[0].username, "tester");
        assert!(claude.service_units.is_empty());
        assert!(codex.processes.is_empty());
        assert_eq!(codex.service_units, ["codex@worker.service"]);
        assert_eq!(tools.len(), 5);
        assert!(tools.iter().all(|tool| tool.category == "assistant"));
    }
}
