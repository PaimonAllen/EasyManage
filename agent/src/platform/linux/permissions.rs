//! Bounded, read-only checks against the directory and privilege rules in docs/index.md.
//! These are entry-point checks, not a recursive audit or an authorization probe.

use crate::platform::common::command::read_command;
use chrono::Utc;
use easymanage_protocol::{PermissionAudit, PermissionCheck, UserStatus};
use std::collections::HashMap;
use std::fs::{self, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

const MAX_HOMES: usize = 64;
const MAX_PROJECTS: usize = 32;
const MAX_SUDOERS_FILES: usize = 64;

fn check(
    category: &str,
    title: &str,
    path: &str,
    status: &str,
    observed: String,
    expected: &str,
    detail: &str,
) -> PermissionCheck {
    PermissionCheck {
        category: category.to_owned(),
        title: title.to_owned(),
        path: path.to_owned(),
        status: status.to_owned(),
        observed,
        expected: expected.to_owned(),
        detail: detail.to_owned(),
    }
}

fn describe(metadata: &Metadata) -> String {
    format!(
        "UID {} · GID {} · {:04o}",
        metadata.uid(),
        metadata.gid(),
        metadata.mode() & 0o7777
    )
}

fn read_bounded(path: &str, limit: u64) -> io::Result<String> {
    let mut content = String::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > limit {
        return Err(io::Error::other("permission file exceeded size limit"));
    }
    Ok(content)
}

fn path_metadata(
    path: &str,
    category: &str,
    title: &str,
    expected: &str,
    directory: bool,
    checks: &mut Vec<PermissionCheck>,
) -> Option<Metadata> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            checks.push(check(
                category,
                title,
                path,
                "warning",
                "符号链接".to_owned(),
                expected,
                "未跟随链接；请人工核对真实目标及权限",
            ));
            None
        }
        Ok(metadata) if directory && !metadata.is_dir() || !directory && !metadata.is_file() => {
            checks.push(check(
                category,
                title,
                path,
                "critical",
                describe(&metadata),
                expected,
                "路径类型不符合规范",
            ));
            None
        }
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            checks.push(check(
                category,
                title,
                path,
                "missing",
                "不存在".to_owned(),
                expected,
                "规范目录或文件尚未配置；不代表权限安全",
            ));
            None
        }
        Err(_) => {
            checks.push(check(
                category,
                title,
                path,
                "unknown",
                "无法读取元数据".to_owned(),
                expected,
                "Agent 当前权限不足或路径不可访问",
            ));
            None
        }
    }
}

fn local_groups() -> HashMap<u32, String> {
    let content = read_bounded("/etc/group", 1024 * 1024).unwrap_or_default();
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            fields.next()?;
            let gid = fields.next()?.parse().ok()?;
            Some((gid, name.to_owned()))
        })
        .collect()
}

fn inspect_local(users: &[UserStatus]) -> (Vec<PermissionCheck>, Vec<(String, String)>, bool) {
    let mut checks = Vec::new();
    let mut acl_paths = Vec::new();
    let mut truncated = false;
    let groups = local_groups();
    let lab_gid = groups
        .iter()
        .find_map(|(gid, name)| (name == "lab-users").then_some(*gid));

    for user in users.iter().filter(|user| user.uid != 0).take(MAX_HOMES) {
        if !user
            .username
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            checks.push(check(
                "home",
                &user.username,
                &user.home,
                "unknown",
                "账户名不符合路径检查规则".to_owned(),
                "/home/用户名",
                "未拼接或访问该账户路径",
            ));
            continue;
        }
        let path = format!("/home/{}", user.username);
        if user.home != path {
            checks.push(check(
                "home",
                &user.username,
                &path,
                "warning",
                format!("账户登记的 home 为 {}", user.home),
                "/home/用户名",
                "规范路径与账户记录不一致；不扫描其他位置",
            ));
            continue;
        }
        if let Some(metadata) = path_metadata(
            &path,
            "home",
            &user.username,
            "本人持有；组与其他用户无权限",
            true,
            &mut checks,
        ) {
            let isolated = metadata.uid() == user.uid && metadata.mode() & 0o077 == 0;
            checks.push(check(
                "home",
                &user.username,
                &path,
                if isolated { "ok" } else { "warning" },
                describe(&metadata),
                "UID 与账户一致，组/其他权限为 0",
                if isolated {
                    "目录入口的 POSIX 权限满足隔离条件"
                } else {
                    "目录所有者或组/其他访问位不符合个人目录隔离要求"
                },
            ));
        }
    }
    if users.iter().filter(|user| user.uid != 0).count() > MAX_HOMES {
        truncated = true;
    }

    if let Some(metadata) = path_metadata(
        "/data/shared",
        "shared",
        "课题组共享目录",
        "lab-users 组 · setgid · 组可读写进入 · 其他不可写",
        true,
        &mut checks,
    ) {
        let mode = metadata.mode();
        let group_known = lab_gid.is_some();
        let valid = metadata.uid() == 0
            && lab_gid == Some(metadata.gid())
            && mode & 0o2000 != 0
            && mode & 0o070 == 0o070
            && mode & 0o002 == 0;
        checks.push(check(
            "shared",
            "共享目录入口",
            "/data/shared",
            if valid {
                "ok"
            } else if group_known {
                "warning"
            } else {
                "unknown"
            },
            describe(&metadata),
            "root:lab-users；建议 2775；组可协作且继承组",
            "只检查目录入口；内部文件不递归扫描",
        ));
        acl_paths.push(("/data/shared".to_owned(), "lab-users".to_owned()));
    }
    if let Some(metadata) = path_metadata(
        "/data/scratch",
        "shared",
        "临时计算目录",
        "lab-users 组 · setgid + sticky · 3777",
        true,
        &mut checks,
    ) {
        let mode = metadata.mode();
        let valid =
            lab_gid == Some(metadata.gid()) && mode & 0o3000 == 0o3000 && mode & 0o777 == 0o777;
        checks.push(check(
            "shared",
            "临时目录入口",
            "/data/scratch",
            if valid {
                "ok"
            } else if lab_gid.is_some() {
                "warning"
            } else {
                "unknown"
            },
            describe(&metadata),
            "lab-users 组；3777（共享写入时必须有 sticky 位）",
            "此处全员可写是文档规定的例外；sticky 位用于限制删除他人文件",
        ));
    }
    if let Some(metadata) = path_metadata(
        "/data/archive",
        "shared",
        "归档目录",
        "不得允许其他用户写入",
        true,
        &mut checks,
    ) {
        let safe = metadata.mode() & 0o002 == 0;
        checks.push(check(
            "shared",
            "归档目录入口",
            "/data/archive",
            if safe { "ok" } else { "critical" },
            describe(&metadata),
            "其他用户不可写；具体授权需按业务复核",
            "规范未规定精确属主与 ACL，仅检查明显危险的全员可写",
        ));
    }
    if let Some(metadata) = path_metadata(
        "/data/projects",
        "project",
        "项目根目录",
        "不可被所有用户写入",
        true,
        &mut checks,
    ) {
        checks.push(check(
            "project",
            "项目根目录入口",
            "/data/projects",
            if metadata.mode() & 0o002 == 0 {
                "ok"
            } else {
                "critical"
            },
            describe(&metadata),
            "其他用户不可写",
            "不递归检查项目内部文件",
        ));
        match fs::read_dir("/data/projects") {
            Ok(entries) => {
                for (index, entry) in entries.enumerate() {
                    if index >= MAX_PROJECTS {
                        truncated = true;
                        break;
                    }
                    let Ok(entry) = entry else {
                        checks.push(check(
                            "project",
                            "项目目录项",
                            "/data/projects",
                            "unknown",
                            "目录项不可读取".to_owned(),
                            "可枚举全部直接子项",
                            "跳过该项，不视为通过",
                        ));
                        continue;
                    };
                    let path = entry.path();
                    let Some(path) = path.to_str() else {
                        truncated = true;
                        continue;
                    };
                    let Some(metadata) = path_metadata(
                        path,
                        "project",
                        "合作项目",
                        "proj-* 组 · setgid · 组可协作 · 其他无权限",
                        true,
                        &mut checks,
                    ) else {
                        continue;
                    };
                    let group = groups.get(&metadata.gid());
                    let visible_members = group.map_or(0, |name| {
                        users
                            .iter()
                            .filter(|user| user.groups.contains(name))
                            .count()
                    });
                    let mode = metadata.mode();
                    let valid = group.is_some_and(|name| name.starts_with("proj-"))
                        && mode & 0o2000 != 0
                        && mode & 0o070 == 0o070
                        && mode & 0o007 == 0;
                    checks.push(check(
                        "project",
                        &entry.file_name().to_string_lossy(),
                        path,
                        if valid {
                            "ok"
                        } else if group.is_none() {
                            "unknown"
                        } else {
                            "warning"
                        },
                        format!(
                            "{} · 组 {} · 可见成员 {} 人",
                            describe(&metadata),
                            group.map(String::as_str).unwrap_or("未知"),
                            visible_members
                        ),
                        "proj-* 组；setgid；组 rwx；其他无权限",
                        "仅检查项目顶层目录；项目成员资格及内部内容需另行核对",
                    ));
                    acl_paths.push((path.to_owned(), group.cloned().unwrap_or_default()));
                }
            }
            Err(_) => checks.push(check(
                "project",
                "项目列表",
                "/data/projects",
                "unknown",
                "无法列出目录".to_owned(),
                "最多检查 32 个直接子目录",
                "Agent 当前权限不足；不能据此判断项目隔离正常",
            )),
        }
    }

    for (path, title, directory, expected, safe_mode) in [
        (
            "/root",
            "root 家目录",
            true,
            "root 持有；组/其他无权限",
            0o077,
        ),
        (
            "/root/.ssh",
            "root SSH 目录",
            true,
            "root 持有；组/其他无权限",
            0o077,
        ),
        (
            "/root/.ssh/authorized_keys",
            "root 授权公钥",
            false,
            "root 持有；组/其他无权限",
            0o077,
        ),
        (
            "/etc/sudoers",
            "sudo 主配置",
            false,
            "root 持有；组/其他不可写",
            0o022,
        ),
        (
            "/etc/sudoers.d",
            "sudo 规则目录",
            true,
            "root 持有；组/其他不可写",
            0o022,
        ),
        (
            "/etc/shadow",
            "shadow 凭据文件",
            false,
            "root 持有；其他无权限、组不可写且不可执行",
            0o037,
        ),
        (
            "/etc/passwd",
            "账户索引文件",
            false,
            "root 持有；组/其他不可写",
            0o022,
        ),
        (
            "/etc/group",
            "用户组索引文件",
            false,
            "root 持有；组/其他不可写",
            0o022,
        ),
    ] {
        if let Some(metadata) = path_metadata(
            path,
            if path.starts_with("/root")
                || path == "/etc/shadow"
                || path == "/etc/passwd"
                || path == "/etc/group"
            {
                "root"
            } else {
                "sudo"
            },
            title,
            expected,
            directory,
            &mut checks,
        ) {
            let safe = metadata.uid() == 0 && metadata.mode() & safe_mode == 0;
            checks.push(check(
                if path.starts_with("/root")
                    || path == "/etc/shadow"
                    || path == "/etc/passwd"
                    || path == "/etc/group"
                {
                    "root"
                } else {
                    "sudo"
                },
                title,
                path,
                if safe { "ok" } else { "critical" },
                describe(&metadata),
                expected,
                "只读检查属主和权限位；不读取凭据内容",
            ));
        }
    }
    if let Some(metadata) = path_metadata(
        "/etc/sudoers.d/lab-admins",
        "sudo",
        "lab-admins 授权规则",
        "root 持有；0440；%lab-admins ALL=(ALL:ALL) ALL",
        false,
        &mut checks,
    ) {
        let safe = metadata.uid() == 0 && metadata.mode() & 0o7777 == 0o440;
        checks.push(check(
            "sudo",
            "lab-admins 规则权限",
            "/etc/sudoers.d/lab-admins",
            if safe { "ok" } else { "critical" },
            describe(&metadata),
            "root 持有；0440",
            "规则内容另列检查；不修改 sudoers",
        ));
        if safe {
            let content = read_bounded("/etc/sudoers.d/lab-admins", 64 * 1024);
            let (status, observed, detail) = match content {
                Ok(content) => {
                    let active: Vec<&str> = content
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                        .collect();
                    if active.len() == 1
                        && active[0].split_whitespace().collect::<Vec<_>>().join(" ")
                            == "%lab-admins ALL=(ALL:ALL) ALL"
                    {
                        (
                            "ok",
                            "规则与操作手册一致",
                            "仅核对该文件的有效文本；不等于完整 sudo 策略验证",
                        )
                    } else {
                        (
                            "warning",
                            "规则与操作手册不一致",
                            "请管理员用 visudo 人工复核；不会在页面展示规则内容",
                        )
                    }
                }
                _ => (
                    "unknown",
                    "Agent 无法读取规则内容",
                    "Basic 模式常见；可提升为 Full 后复查",
                ),
            };
            checks.push(check(
                "sudo",
                "lab-admins 规则内容",
                "/etc/sudoers.d/lab-admins",
                status,
                observed.to_owned(),
                "%lab-admins ALL=(ALL:ALL) ALL",
                detail,
            ));
        }
    }

    let aliases: Vec<&str> = users
        .iter()
        .filter(|user| user.uid == 0 && user.username != "root")
        .map(|user| user.username.as_str())
        .collect();
    let root_visible = users
        .iter()
        .any(|user| user.uid == 0 && user.username == "root");
    checks.push(check(
        "root",
        "UID 0 账户",
        "/etc/passwd",
        if !root_visible {
            "unknown"
        } else if aliases.is_empty() {
            "ok"
        } else {
            "critical"
        },
        if !root_visible {
            "账户清单不可用".to_owned()
        } else if aliases.is_empty() {
            "仅 root".to_owned()
        } else {
            format!("额外 UID 0：{}", aliases.join(", "))
        },
        "仅 root 应具有 UID 0",
        "来自账户清单；不读取 shadow 或密码",
    ));
    let admin_group_present = groups.values().any(|name| name == "lab-admins");
    checks.push(check(
        "sudo",
        "管理员组",
        "/etc/group",
        if admin_group_present { "ok" } else { "unknown" },
        if admin_group_present {
            "lab-admins 已存在".to_owned()
        } else {
            "本地组清单未找到 lab-admins".to_owned()
        },
        "管理员由 lab-admins 组授权",
        "仅核对本地组定义；NSS/LDAP 授权需人工核对",
    ));
    for user in users.iter().filter(|user| {
        user.uid != 0
            && user
                .groups
                .iter()
                .any(|group| group == "sudo" || group == "wheel")
            && !user.groups.iter().any(|group| group == "lab-admins")
    }) {
        checks.push(check(
            "sudo",
            &user.username,
            "/etc/group",
            "warning",
            "属于 sudo/wheel，但不属于 lab-admins".to_owned(),
            "管理员按规范归入 lab-admins",
            "也可能是合法的历史授权，需管理员确认；未推断其实际 sudo 能力",
        ));
    }
    let mut sudo_rule_paths = vec![
        "/etc/sudoers".to_owned(),
        "/etc/sudoers.d/lab-admins".to_owned(),
    ];
    let mut sudo_rules_truncated = false;
    let mut sudo_rules_listed = false;
    let sudo_dir_real = fs::symlink_metadata("/etc/sudoers.d")
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
    if let Some(Ok(entries)) = sudo_dir_real.then(|| fs::read_dir("/etc/sudoers.d")) {
        sudo_rules_listed = true;
        for (index, entry) in entries.enumerate() {
            if index >= MAX_SUDOERS_FILES {
                truncated = true;
                sudo_rules_truncated = true;
                break;
            }
            let Ok(entry) = entry else {
                checks.push(check(
                    "sudo",
                    "sudo 附加规则项",
                    "/etc/sudoers.d",
                    "unknown",
                    "目录项不可读取".to_owned(),
                    "可枚举全部直接子项",
                    "跳过该项，不视为通过",
                ));
                continue;
            };
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Some(path) = path.to_str() else {
                continue;
            };
            if path == "/etc/sudoers.d/lab-admins" {
                continue;
            }
            if let Some(metadata) = path_metadata(
                path,
                "sudo",
                "sudo 附加规则权限",
                "root 持有；组/其他不可写",
                false,
                &mut checks,
            ) {
                sudo_rule_paths.push(path.to_owned());
                let safe = metadata.uid() == 0 && metadata.mode() & 0o022 == 0;
                checks.push(check(
                    "sudo",
                    "sudo 附加规则权限",
                    path,
                    if safe { "ok" } else { "critical" },
                    describe(&metadata),
                    "root 持有；组/其他不可写",
                    "不解释该文件内的授权策略",
                ));
            }
        }
    } else {
        checks.push(check(
            "sudo",
            "sudo 附加规则列表",
            "/etc/sudoers.d",
            "unknown",
            "Agent 无法列出规则文件".to_owned(),
            "能够列出并核对规则文件属主与权限",
            "Basic 模式或目录权限限制；不能据此判断其他规则安全",
        ));
    }
    let mut unreadable_rules = false;
    let mut passwordless_rules = Vec::new();
    for path in &sudo_rule_paths {
        if !fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            unreadable_rules = true;
            continue;
        }
        match read_bounded(path, 64 * 1024) {
            Ok(content)
                if content
                    .lines()
                    .any(|line| line.split('#').next().unwrap_or("").contains("NOPASSWD:")) =>
            {
                passwordless_rules.push(path.as_str());
            }
            Ok(_) => {}
            Err(_) => unreadable_rules = true,
        }
    }
    let (status, observed) = if !passwordless_rules.is_empty() {
        (
            "warning",
            format!("发现 NOPASSWD 字面规则：{}", passwordless_rules.join(", ")),
        )
    } else if unreadable_rules || sudo_rules_truncated || !sudo_rules_listed {
        ("unknown", "部分 sudo 规则不可读取或列表不完整".to_owned())
    } else {
        ("ok", "已检查的文件未出现 NOPASSWD 字样".to_owned())
    };
    checks.push(check(
        "sudo",
        "免密 sudo 字面检查",
        "/etc/sudoers及/etc/sudoers.d",
        status,
        observed,
        "通常应要求管理员输入密码",
        "仅扫描可见文件的非注释文本；不解析 include、LDAP 或 sudoers 完整语法",
    ));
    (checks, acl_paths, truncated)
}

fn parse_acl(output: &str) -> HashMap<String, Vec<String>> {
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    let mut path = None;
    for line in output.lines() {
        if let Some(name) = line.strip_prefix("# file: ") {
            path = Some(name.to_owned());
            result.entry(name.to_owned()).or_default();
        } else if let Some(name) = &path {
            if line.is_empty() {
                path = None;
            } else if !line.starts_with('#') {
                result
                    .entry(name.clone())
                    .or_default()
                    .push(line.split('#').next().unwrap_or("").trim().to_owned());
            }
        }
    }
    result
}

fn acl_extra_subjects(entries: &[String], group: &str) -> bool {
    entries.iter().any(|entry| {
        let normalized = entry.strip_prefix("default:").unwrap_or(entry);
        let fields: Vec<&str> = normalized.split(':').collect();
        fields.len() >= 3
            && ((fields[0] == "user" && !fields[1].is_empty())
                || (fields[0] == "group" && !fields[1].is_empty() && fields[1] != group))
    })
}

pub async fn collect(users: &[UserStatus]) -> PermissionAudit {
    let users = users.to_vec();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || inspect_local(&users)),
    )
    .await;
    let (mut checks, acl_paths, truncated) = match result {
        Ok(Ok(value)) => value,
        _ => (
            vec![check(
                "root",
                "权限采集",
                "—",
                "unknown",
                "检查任务失败".to_owned(),
                "只读检查正常完成",
                "不将失败视为安全",
            )],
            Vec::new(),
            false,
        ),
    };
    let acl = if acl_paths.is_empty() {
        Some(HashMap::new())
    } else {
        let mut args = vec!["-p", "--"];
        args.extend(acl_paths.iter().map(|(path, _)| path.as_str()));
        read_command("getfacl", &args, 256 * 1024)
            .await
            .ok()
            .map(|output| parse_acl(&output))
    };
    for (path, group) in acl_paths {
        let category = if path == "/data/shared" {
            "shared"
        } else {
            "project"
        };
        let title = if category == "shared" {
            "共享目录 ACL 继承"
        } else {
            "项目目录 ACL 主体"
        };
        let entries = acl.as_ref().and_then(|map| map.get(&path));
        let (status, observed, detail) = match entries {
            None => (
                "unknown",
                "ACL 不可读取",
                "getfacl 不可用、超时或某路径无访问权限；不能确认 ACL 授权",
            ),
            Some(entries) if entries.is_empty() => {
                ("unknown", "ACL 输出为空", "不能把空输出解释为无额外授权")
            }
            Some(entries) if acl_extra_subjects(entries, &group) => (
                "warning",
                "存在额外命名用户/组 ACL",
                "可能扩大授权范围，请管理员核对有效权限及项目成员",
            ),
            Some(entries)
                if category == "shared"
                    && !(entries
                        .iter()
                        .any(|entry| entry == "default:group:lab-users:rwx")
                        && entries.iter().any(|entry| entry == "default:mask::rwx")) =>
            {
                (
                    "warning",
                    "缺少文档要求的默认 ACL",
                    "新建内容可能不能持续由 lab-users 共享写入",
                )
            }
            Some(_) => (
                "ok",
                "未发现偏离规范的目录 ACL",
                "仅检查目录自身 ACL；不递归审计子目录和文件",
            ),
        };
        checks.push(check(
            category,
            title,
            &path,
            status,
            observed.to_owned(),
            if category == "shared" {
                "默认 lab-users:rwx 与 mask:rwx"
            } else {
                "无额外未审核主体"
            },
            detail,
        ));
    }
    PermissionAudit {
        collected_at: Utc::now(),
        checks,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::{acl_extra_subjects, parse_acl};

    #[test]
    fn parses_acl_and_flags_unexpected_project_user() {
        let acl = parse_acl(
            "# file: /data/projects/demo\nuser::rwx\nuser:outsider:rwx\ngroup::rwx\nother::---\n\n",
        );
        assert!(acl_extra_subjects(&acl["/data/projects/demo"], "proj-demo"));
    }

    #[test]
    fn default_project_group_is_not_an_extra_subject() {
        let entries = vec![
            "group::rwx".to_owned(),
            "default:group:proj-demo:rwx".to_owned(),
        ];
        assert!(!acl_extra_subjects(&entries, "proj-demo"));
    }
}
