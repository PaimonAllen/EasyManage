use super::ai_tools;
use super::block_devices;
use super::overlay_networks;
use super::programming_tools;
use crate::platform::common::command::read_command;
use chrono::{DateTime, Utc};
use easymanage_protocol::{
    AgentPrivilege, AiToolProcess, AiToolStatus, BlockDevice, CollectionIssue, HostSecurity,
    NetworkInterface, OverlayNetworkStatus, ProxyService, RouteInfo, ServiceStatus,
    SoftwarePackage, UserSessionEvidence, UserStatus,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const INVENTORY_LIMIT: u64 = 1024 * 1024;
const PROCESS_LIMIT: usize = 8192;
const PROXY_LIMIT: usize = 128;

#[derive(Clone)]
pub struct ExtendedInventory {
    pub collected_at: DateTime<Utc>,
    pub privilege: AgentPrivilege,
    pub users: Vec<UserStatus>,
    pub block_devices: Vec<BlockDevice>,
    pub interfaces: Vec<NetworkInterface>,
    pub overlay_networks: Vec<OverlayNetworkStatus>,
    pub routes: Vec<RouteInfo>,
    pub proxy_services: Vec<ProxyService>,
    pub software_packages: Vec<SoftwarePackage>,
    pub running_services: Vec<ServiceStatus>,
    pub host_security: HostSecurity,
    pub ai_tools: Vec<AiToolStatus>,
    pub programming_tools: Vec<easymanage_protocol::ProgrammingToolStatus>,
    pub issues: Vec<CollectionIssue>,
}

impl ExtendedInventory {
    pub async fn collect(users_enabled: bool, proxy_enabled: bool) -> Self {
        let (
            accounts,
            sessions,
            logind_sessions,
            interfaces,
            routes_v4,
            routes_v6,
            packages,
            services,
            block_devices,
            overlay_networks,
        ) = tokio::join!(
            read_command("getent", &["passwd"], INVENTORY_LIMIT),
            async {
                if users_enabled {
                    read_command("who", &[], 256 * 1024).await
                } else {
                    Ok(String::new())
                }
            },
            async {
                if users_enabled {
                    read_command(
                        "loginctl",
                        &["list-sessions", "--no-legend", "--no-pager"],
                        256 * 1024,
                    )
                    .await
                } else {
                    Ok(String::new())
                }
            },
            read_command("ip", &["-j", "-d", "address", "show"], INVENTORY_LIMIT),
            read_command(
                "ip",
                &["-j", "route", "show", "table", "all"],
                INVENTORY_LIMIT
            ),
            read_command(
                "ip",
                &["-j", "-6", "route", "show", "table", "all"],
                INVENTORY_LIMIT
            ),
            read_command(
                "dpkg-query",
                &["-W", "-f=${Package}\\t${Version}\\n"],
                INVENTORY_LIMIT
            ),
            read_command(
                "systemctl",
                &[
                    "list-units",
                    "--type=service",
                    "--state=running",
                    "--no-legend",
                    "--no-pager",
                    "--plain"
                ],
                256 * 1024
            ),
            read_command(
                "lsblk",
                &[
                    "-J",
                    "-b",
                    "-o",
                    "NAME,KNAME,PATH,TYPE,SIZE,FSTYPE,LABEL,MOUNTPOINTS,MODEL,TRAN,ROTA,RM,RO,PTTYPE,PARTLABEL"
                ],
                INVENTORY_LIMIT,
            ),
            overlay_networks::collect(),
        );
        // /proc file traversal is synchronous and must not block report/shutdown tasks.
        let inventory = tokio::task::spawn_blocking(move || {
            Self::from_sources(
                accounts,
                sessions,
                logind_sessions,
                interfaces,
                routes_v4,
                routes_v6,
                packages,
                services,
                block_devices,
                users_enabled,
                proxy_enabled,
            )
        })
        .await
        .unwrap_or_else(|_| Self {
            collected_at: Utc::now(),
            privilege: current_privilege(&HashMap::new()),
            users: Vec::new(),
            block_devices: Vec::new(),
            interfaces: Vec::new(),
            overlay_networks: Vec::new(),
            routes: Vec::new(),
            proxy_services: Vec::new(),
            software_packages: Vec::new(),
            running_services: Vec::new(),
            host_security: collect_host_security(),
            ai_tools: Vec::new(),
            programming_tools: Vec::new(),
            issues: vec![issue("inventory", "扩展信息采集任务失败")],
        });
        Self {
            overlay_networks,
            ..inventory
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "bounded inventory parsers share a single issue accumulator"
    )]
    fn from_sources(
        accounts: io::Result<String>,
        sessions: io::Result<String>,
        logind_sessions: io::Result<String>,
        interfaces: io::Result<String>,
        routes_v4: io::Result<String>,
        routes_v6: io::Result<String>,
        packages: io::Result<String>,
        services: io::Result<String>,
        block_devices: io::Result<String>,
        users_enabled: bool,
        proxy_enabled: bool,
    ) -> Self {
        let mut issues = Vec::new();
        let passwd = accounts.unwrap_or_else(|_| {
            issues.push(issue("users", "NSS 账户查询不可用，仅显示可读取的本地账户"));
            issues.push(issue(
                "ai_tools",
                "NSS 账户查询失败，AI 安装目录仅从可读取的本地账户发现，结果可能不完整",
            ));
            read_small("/etc/passwd", INVENTORY_LIMIT).unwrap_or_default()
        });
        let (minimum_uid, maximum_uid) = uid_range();
        let mut owners = HashMap::new();
        let mut users = Vec::new();
        let mut ai_homes = Vec::new();
        for line in passwd.lines().take(4096) {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() != 7 {
                continue;
            }
            let (Ok(uid), Ok(gid)) = (fields[2].parse::<u32>(), fields[3].parse::<u32>()) else {
                continue;
            };
            owners.insert(uid, fields[0].to_owned());
            if uid == 0
                || (minimum_uid..=maximum_uid).contains(&uid)
                || fields[5].starts_with("/home/")
            {
                ai_homes.push(fields[5].to_owned());
            }
            if uid != 0 && !(minimum_uid..=maximum_uid).contains(&uid) {
                continue;
            }
            users.push(UserStatus {
                username: fields[0].to_owned(),
                uid,
                gid,
                home: fields[5].to_owned(),
                shell: fields[6].to_owned(),
                groups: Vec::new(),
                sessions: None,
                session_sources: Vec::new(),
                session_evidence: None,
                process_count: None,
                locked: None,
                account_status: users_enabled.then(|| account_status(fields[6])).flatten(),
            });
        }
        if passwd.lines().count() > 4096 {
            issues.push(issue("users", "账户数量超出 4096 条采集上限，列表不完整"));
            issues.push(issue(
                "ai_tools",
                "账户清单超过 4096 条上限，AI 安装目录扫描可能不完整",
            ));
        }
        if users_enabled {
            fill_groups(&mut users);
        }
        let block_devices = match block_devices.and_then(|output| block_devices::parse(&output)) {
            Ok(devices) => devices,
            Err(_) => {
                issues.push(issue("storage", "块设备树不可读取；仅显示已挂载文件系统"));
                Vec::new()
            }
        };
        let terminal_sessions = if !users_enabled {
            None
        } else {
            match sessions {
                Ok(output) => Some(parse_terminal_sessions(&output)),
                Err(_) => {
                    issues.push(issue("sessions", "utmp 会话不可读取"));
                    None
                }
            }
        };
        let system_sessions = if !users_enabled {
            None
        } else {
            match logind_sessions {
                Ok(output) => parse_logind_sessions(&output).or_else(|| {
                    issues.push(issue("sessions", "logind 会话输出不可解析"));
                    None
                }),
                Err(_) => {
                    issues.push(issue("sessions", "logind 会话不可读取"));
                    None
                }
            }
        };
        let privilege = current_privilege(&owners);
        let mut ai_processes = HashMap::new();
        let (proxy_services, ssh_sessions, processes_complete) = process_inventory(
            &mut users,
            &owners,
            &mut ai_processes,
            &mut issues,
            users_enabled,
            proxy_enabled,
        );
        if users_enabled {
            for user in &mut users {
                let terminal = terminal_sessions
                    .as_ref()
                    .and_then(|counts| counts.get(user.username.as_str()))
                    .copied()
                    .unwrap_or(0);
                let system = system_sessions
                    .as_ref()
                    .and_then(|counts| counts.get(&user.uid))
                    .copied()
                    .unwrap_or(0);
                let ssh = ssh_sessions.get(&user.uid).copied().unwrap_or(0);
                user.session_evidence = Some(UserSessionEvidence {
                    terminal_records: terminal_sessions.as_ref().map(|_| terminal),
                    logind_sessions: system_sessions.as_ref().map(|_| system),
                    ssh_processes: processes_complete.then_some(ssh),
                });
                if terminal > 0 {
                    user.session_sources.push("terminal".to_owned());
                }
                if system > 0 {
                    user.session_sources.push("logind".to_owned());
                }
                if ssh > 0 {
                    user.session_sources.push("ssh".to_owned());
                }
                user.sessions = if terminal_sessions.is_some()
                    || system_sessions.is_some()
                    || processes_complete
                    || ssh > 0
                {
                    // Sources overlap. An SSH process proves presence but is not a distinct
                    // session count, so it contributes at most one here.
                    Some(terminal.max(system).max(u32::from(ssh > 0)))
                } else {
                    None
                };
            }
            issues.push(issue(
            "users",
            "账户基础状态只核验登录 Shell；会话综合 utmp、logind 和 SSH 进程，不读取口令或 shadow",
        ));
        }
        if privilege.mode == "basic" {
            issues.push(issue(
                "privilege",
                "普通用户模式：其他用户的套接字、进程及设备信息可能不可见",
            ));
        }
        let interfaces = if let Ok(value) = interfaces.and_then(|data| parse_interfaces(&data)) {
            value
        } else {
            issues.push(issue(
                "network",
                "ip 地址查询失败；仅显示 sysfs 网卡，地址和 VLAN 详情可能缺失",
            ));
            fallback_interfaces()
        };
        let mut routes = Vec::new();
        for (source, ipv6) in [(routes_v4, false), (routes_v6, true)] {
            match source.and_then(|data| parse_routes(&data, ipv6)) {
                Ok(mut values) => routes.append(&mut values),
                Err(_) => issues.push(issue(
                    "network",
                    if ipv6 {
                        "IPv6 路由不可读取"
                    } else {
                        "IPv4 路由不可读取"
                    },
                )),
            }
        }
        let software_packages = if let Ok(output) = packages {
            output
                .lines()
                .take(4096)
                .filter_map(|line| {
                    let (name, version) = line.split_once('\t')?;
                    if name.is_empty() || version.is_empty() {
                        return None;
                    }
                    Some(SoftwarePackage {
                        name: name.to_owned(),
                        version: version.to_owned(),
                    })
                })
                .collect()
        } else {
            issues.push(issue("software", "dpkg 软件包清单不可读取"));
            Vec::new()
        };
        let running_services = if let Ok(output) = services {
            output
                .lines()
                .take(256)
                .filter_map(|line| {
                    let name = line
                        .split_whitespace()
                        .find(|field| field.ends_with(".service"))?;
                    Some(ServiceStatus {
                        name: name.to_owned(),
                        state: "running".to_owned(),
                    })
                })
                .collect()
        } else {
            issues.push(issue("software", "systemd 服务清单不可读取"));
            issues.push(issue(
                "ai_tools",
                "systemd 服务清单不可读取，AI 服务运行状态可能不完整",
            ));
            Vec::new()
        };
        let agent_home = users
            .iter()
            .find(|user| user.uid == privilege.uid)
            .map(|user| user.home.as_str());
        let ai_tools =
            ai_tools::inventory(&ai_homes, &ai_processes, &running_services, &mut issues);
        let programming_tools = programming_tools::inventory(agent_home, &software_packages);
        Self {
            collected_at: Utc::now(),
            privilege,
            users,
            block_devices,
            interfaces,
            overlay_networks: Vec::new(),
            routes,
            proxy_services,
            software_packages,
            running_services,
            host_security: collect_host_security(),
            ai_tools,
            programming_tools,
            issues,
        }
    }

    pub fn clear_user_status(&mut self) {
        // Base account IDs/home paths remain inputs to the separate permission and
        // tool checks. No groups, sessions, process counts or shell status are retained.
        for user in &mut self.users {
            user.groups.clear();
            user.sessions = None;
            user.session_sources.clear();
            user.session_evidence = None;
            user.process_count = None;
            user.account_status = None;
        }
        self.issues
            .retain(|item| !matches!(item.component.as_str(), "users" | "sessions"));
    }
}

fn collect_host_security() -> HostSecurity {
    let apparmor_enabled = read_small("/sys/module/apparmor/parameters/enabled", 64)
        .ok()
        .and_then(|value| match value.trim() {
            "Y" => Some(true),
            "N" => Some(false),
            _ => None,
        });
    let aslr_mode = read_small("/proc/sys/kernel/randomize_va_space", 64)
        .ok()
        .and_then(|value| value.trim().parse().ok());
    let ptrace_scope = read_small("/proc/sys/kernel/yama/ptrace_scope", 64)
        .ok()
        .and_then(|value| value.trim().parse().ok());
    let kernel_lockdown = read_small("/sys/kernel/security/lockdown", 128)
        .ok()
        .and_then(|value| {
            value
                .split_whitespace()
                .find_map(|part| part.strip_prefix('[')?.strip_suffix(']'))
                .filter(|mode| matches!(*mode, "none" | "integrity" | "confidentiality"))
                .map(str::to_owned)
        });
    HostSecurity {
        apparmor_enabled,
        aslr_mode,
        ptrace_scope,
        kernel_lockdown,
    }
}

pub fn effective_uid() -> Option<u32> {
    let status = read_small("/proc/self/status", 64 * 1024).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("Uid:")?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    })
}

fn current_privilege(owners: &HashMap<u32, String>) -> AgentPrivilege {
    let uid = effective_uid().unwrap_or(u32::MAX);
    AgentPrivilege {
        mode: if uid == 0 { "full" } else { "basic" }.to_owned(),
        username: owners.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
        uid,
    }
}

fn uid_range() -> (u32, u32) {
    let definitions = read_small("/etc/login.defs", 128 * 1024).unwrap_or_default();
    let mut minimum = 1000;
    let mut maximum = 60000;
    for line in definitions.lines() {
        let mut fields = line.split_whitespace();
        match (
            fields.next(),
            fields.next().and_then(|value| value.parse().ok()),
        ) {
            (Some("UID_MIN"), Some(value)) => minimum = value,
            (Some("UID_MAX"), Some(value)) => maximum = value,
            _ => {}
        }
    }
    (minimum, maximum.max(minimum))
}

fn account_status(shell: &str) -> Option<String> {
    let path = Path::new(shell);
    if !path.is_absolute() {
        return Some("invalid".to_owned());
    }
    if path
        .file_name()
        .is_some_and(|name| name == "nologin" || name == "false")
    {
        return Some("restricted".to_owned());
    }
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
            Some("normal".to_owned())
        }
        Ok(_) => Some("invalid".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some("invalid".to_owned()),
        Err(_) => None,
    }
}

fn parse_terminal_sessions(output: &str) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for username in output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
    {
        *counts.entry(username.to_owned()).or_insert(0) += 1;
    }
    counts
}

fn parse_logind_sessions(output: &str) -> Option<HashMap<u32, u32>> {
    let mut counts = HashMap::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        if line.trim_end().ends_with("sessions listed.")
            || line.trim_end().ends_with("session listed.")
            || line.trim() == "No sessions."
        {
            continue;
        }
        let uid = line.split_whitespace().nth(1)?.parse::<u32>().ok()?;
        *counts.entry(uid).or_insert(0) += 1;
    }
    Some(counts)
}

fn fill_groups(users: &mut [UserStatus]) {
    let groups = read_small("/etc/group", INVENTORY_LIMIT).unwrap_or_default();
    for line in groups.lines().take(4096) {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() != 4 {
            continue;
        }
        let Ok(gid) = fields[2].parse::<u32>() else {
            continue;
        };
        let members: HashSet<&str> = fields[3].split(',').collect();
        for user in users.iter_mut() {
            if user.gid == gid || members.contains(user.username.as_str()) {
                user.groups.push(fields[0].to_owned());
            }
        }
    }
}

fn process_inventory(
    users: &mut [UserStatus],
    owners: &HashMap<u32, String>,
    ai_processes: &mut HashMap<&'static str, Vec<AiToolProcess>>,
    issues: &mut Vec<CollectionIssue>,
    users_enabled: bool,
    proxy_enabled: bool,
) -> (Vec<ProxyService>, HashMap<u32, u32>, bool) {
    let Ok(entries) = fs::read_dir("/proc") else {
        issues.push(issue("processes", "/proc 不可读取；进程和代理服务列表未知"));
        return (Vec::new(), HashMap::new(), false);
    };
    // Process enumeration is shared with users/Agent tools, but socket tables
    // and per-proxy fd traversal are exclusively for proxy monitoring.
    let sockets = if proxy_enabled {
        listening_sockets()
    } else {
        HashMap::new()
    };
    let mut counts = HashMap::<u32, u32>::new();
    let mut services = Vec::new();
    let mut partial = proc_hides_processes();
    let mut denied_fds = false;
    let mut ai_partial = false;
    let mut ssh_sessions = HashMap::<u32, u32>::new();
    for (index, (pid, entry)) in entries
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.file_name().to_str()?.parse::<u32>().ok()?, entry)))
        .enumerate()
    {
        if index >= PROCESS_LIMIT {
            partial = true;
            break;
        }
        let status = match read_small(entry.path().join("status"), 64 * 1024) {
            Ok(status) => status,
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    partial = true;
                }
                continue;
            }
        };
        let Some(uid) = status.lines().find_map(|line| {
            line.strip_prefix("Uid:")?
                .split_whitespace()
                .next()?
                .parse::<u32>()
                .ok()
        }) else {
            continue;
        };
        if users_enabled {
            *counts.entry(uid).or_default() += 1;
        }
        let name = match read_small(entry.path().join("comm"), 256) {
            Ok(name) => name.trim().to_owned(),
            Err(error) => {
                ai_partial |= error.kind() != io::ErrorKind::NotFound;
                String::new()
            }
        };
        if users_enabled && uid != 0 && matches!(name.as_str(), "sshd" | "sshd-session") {
            *ssh_sessions.entry(uid).or_default() += 1;
        }
        ai_partial |= record_ai_process(ai_processes, owners, &name, pid, uid);
        if !proxy_enabled || !is_proxy(&name) {
            continue;
        }
        if services.len() >= PROXY_LIMIT {
            partial = true;
            continue;
        }
        let (listen_addresses, incomplete) = process_listeners(&entry.path(), &sockets);
        denied_fds |= incomplete;
        services.push(ProxyService {
            name,
            pid,
            username: owners.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
            uid,
            listen_addresses,
        });
    }
    for user in users {
        user.process_count = if partial || !users_enabled {
            None
        } else {
            Some(*counts.get(&user.uid).unwrap_or(&0))
        };
    }
    if partial {
        issues.push(issue(
            "processes",
            "部分进程不可读取或数量达到上限；进程计数未知，服务列表可能不完整",
        ));
    }
    if partial || ai_partial {
        issues.push(issue(
            "ai_tools",
            "进程清单不可完整读取或超过上限；AI 工具运行状态可能不完整",
        ));
    }
    if denied_fds {
        issues.push(issue(
            "proxy",
            "部分代理服务的监听端口不可读取或达到 512 个文件描述符上限",
        ));
    }
    if proxy_enabled {
        issues.push(issue(
            "proxy",
            "仅识别可见进程名对应的代理服务及当前网络命名空间；不读取命令行、环境变量、代理配置或订阅",
        ));
    }
    services.sort_by(|left, right| {
        (left.uid, &left.name, left.pid).cmp(&(right.uid, &right.name, right.pid))
    });
    (services, ssh_sessions, !partial)
}

fn record_ai_process(
    observed: &mut HashMap<&'static str, Vec<AiToolProcess>>,
    owners: &HashMap<u32, String>,
    process_name: &str,
    pid: u32,
    uid: u32,
) -> bool {
    let Some(tool) = ai_tools::recognize(process_name) else {
        return false;
    };
    let processes = observed.entry(tool).or_default();
    if processes.len() >= 32 {
        return true;
    }
    processes.push(AiToolProcess {
        pid,
        uid,
        username: owners.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
    });
    false
}

fn proc_hides_processes() -> bool {
    read_small("/proc/mounts", INVENTORY_LIMIT).is_ok_and(|mounts| {
        mounts.lines().any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            fields.get(1) == Some(&"/proc")
                && fields.get(3).is_some_and(|options| {
                    options.split(',').any(|option| {
                        option
                            .strip_prefix("hidepid=")
                            .is_some_and(|value| value != "0" && value != "off")
                    })
                })
        })
    })
}

fn process_listeners(path: &Path, sockets: &HashMap<String, String>) -> (Vec<String>, bool) {
    let mut addresses = Vec::new();
    let Ok(fds) = fs::read_dir(path.join("fd")) else {
        return (addresses, true);
    };
    let mut incomplete = false;
    for (index, fd) in fds.filter_map(Result::ok).enumerate() {
        if index >= 512 {
            incomplete = true;
            break;
        }
        let Ok(target) = fs::read_link(fd.path()) else {
            continue;
        };
        if let Some(inode) = target
            .to_str()
            .and_then(|value| value.strip_prefix("socket:["))
            .and_then(|value| value.strip_suffix(']'))
            && let Some(address) = sockets.get(inode)
        {
            addresses.push(address.clone());
        }
    }
    addresses.sort();
    addresses.dedup();
    (addresses, incomplete)
}

fn is_proxy(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "clash"
            | "clash-meta"
            | "clash-premium"
            | "mihomo"
            | "sing-box"
            | "v2ray"
            | "xray"
            | "ss-local"
            | "ss-server"
            | "sslocal"
            | "ssserver"
            | "trojan"
            | "trojan-go"
            | "gost"
            | "privoxy"
            | "tinyproxy"
            | "squid"
            | "brook"
            | "hysteria"
            | "hysteria2"
    )
}

fn listening_sockets() -> HashMap<String, String> {
    let mut result = HashMap::new();
    for (file, protocol, ipv6) in [
        ("tcp", "tcp", false),
        ("tcp6", "tcp", true),
        ("udp", "udp", false),
        ("udp6", "udp", true),
    ] {
        let Ok(content) = read_small(format!("/proc/net/{file}"), 4 * INVENTORY_LIMIT) else {
            continue;
        };
        for line in content.lines().skip(1).take(16384) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10
                || (protocol == "tcp" && fields[3] != "0A")
                || (protocol == "udp" && fields[3] != "07")
            {
                continue;
            }
            if let Some(address) = socket_address(fields[1], ipv6) {
                result.insert(fields[9].to_owned(), format!("{protocol}://{address}"));
            }
        }
    }
    result
}

fn socket_address(value: &str, ipv6: bool) -> Option<String> {
    let (address, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    if ipv6 {
        if address.len() != 32 {
            return None;
        }
        let mut bytes = [0_u8; 16];
        for index in 0..4 {
            let word = u32::from_str_radix(&address[index * 8..index * 8 + 8], 16).ok()?;
            bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_ne_bytes());
        }
        Some(format!("[{}]:{port}", Ipv6Addr::from(bytes)))
    } else {
        Some(format!(
            "{}:{port}",
            Ipv4Addr::from(u32::from_str_radix(address, 16).ok()?.to_ne_bytes())
        ))
    }
}

fn parse_interfaces(data: &str) -> io::Result<Vec<NetworkInterface>> {
    let values: Vec<Value> = serde_json::from_str(data).map_err(io::Error::other)?;
    Ok(values
        .iter()
        .take(512)
        .filter_map(|value| {
            let name = value["ifname"].as_str()?.to_owned();
            let reported_kind = value
                .pointer("/linkinfo/info_kind")
                .and_then(Value::as_str)
                .unwrap_or("");
            let addresses = value["addr_info"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|address| {
                    Some(format!(
                        "{}/{}",
                        address["local"].as_str()?,
                        address["prefixlen"].as_u64()?
                    ))
                })
                .collect();
            Some(NetworkInterface {
                kind: interface_kind(&name, reported_kind),
                name,
                state: value["operstate"]
                    .as_str()
                    .unwrap_or("UNKNOWN")
                    .to_lowercase(),
                addresses,
                mac_address: value["address"].as_str().map(str::to_owned),
                mtu: value["mtu"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok()),
                vlan_id: value
                    .pointer("/linkinfo/info_data/id")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                master: value["master"].as_str().map(str::to_owned),
            })
        })
        .collect())
}

fn interface_kind(name: &str, reported: &str) -> String {
    let sysfs = Path::new("/sys/class/net").join(name);
    classify_interface_kind(
        name,
        reported,
        sysfs.join("wireless").exists() || sysfs.join("phy80211").exists(),
        sysfs.join("device").exists(),
    )
}

fn classify_interface_kind(name: &str, reported: &str, wireless: bool, physical: bool) -> String {
    if name.starts_with("zt") {
        "zerotier".to_owned()
    } else if name.starts_with("tailscale") {
        "tailscale".to_owned()
    } else if wireless {
        "wifi".to_owned()
    } else if !reported.is_empty() {
        reported.to_owned()
    } else if name == "lo" {
        "loopback".to_owned()
    } else if name.starts_with("tun") {
        "tun".to_owned()
    } else if name.starts_with("tap") {
        "tap".to_owned()
    } else if physical {
        "ethernet".to_owned()
    } else {
        "virtual".to_owned()
    }
}

fn fallback_interfaces() -> Vec<NetworkInterface> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .take(512)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            NetworkInterface {
                kind: interface_kind(&name, ""),
                name,
                state: read_small(entry.path().join("operstate"), 64)
                    .unwrap_or_else(|_| "unknown".to_owned())
                    .trim()
                    .to_owned(),
                addresses: Vec::new(),
                mac_address: read_small(entry.path().join("address"), 128)
                    .ok()
                    .map(|value| value.trim().to_owned()),
                mtu: read_small(entry.path().join("mtu"), 64)
                    .ok()
                    .and_then(|value| value.trim().parse().ok()),
                vlan_id: None,
                master: None,
            }
        })
        .collect()
}

fn parse_routes(data: &str, ipv6: bool) -> io::Result<Vec<RouteInfo>> {
    let values: Vec<Value> = serde_json::from_str(data).map_err(io::Error::other)?;
    Ok(values
        .iter()
        .take(1024)
        .map(|value| RouteInfo {
            destination: match value["dst"].as_str().unwrap_or("default") {
                "default" => if ipv6 { "::/0" } else { "0.0.0.0/0" }.to_owned(),
                value => value.to_owned(),
            },
            gateway: value["gateway"].as_str().map(str::to_owned),
            interface: value["dev"].as_str().map(str::to_owned),
            metric: value["metric"]
                .as_u64()
                .and_then(|value| u32::try_from(value).ok()),
        })
        .collect())
}

fn read_small(path: impl AsRef<Path>, limit: u64) -> io::Result<String> {
    let mut content = String::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut content)?;
    if u64::try_from(content.len()).unwrap_or(u64::MAX) > limit {
        return Err(io::Error::other("inventory file exceeded safety limit"));
    }
    Ok(content)
}

pub fn issue(component: &str, message: &str) -> CollectionIssue {
    CollectionIssue {
        component: component.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        account_status, interface_kind, parse_interfaces, parse_logind_sessions, parse_routes,
        parse_terminal_sessions, socket_address,
    };

    #[test]
    fn ai_installation_from_another_account_is_included_in_extended_inventory() {
        use std::{fs, os::unix::fs::PermissionsExt};
        // This fixture writes only in the test namespace's private /home tmpfs.
        let mounts = fs::read_to_string("/proc/self/mountinfo").unwrap();
        assert!(
            mounts.lines().any(|line| {
                line.split_whitespace().nth(4) == Some("/home")
                    && line
                        .split_once(" - ")
                        .is_some_and(|(_, fs)| fs.starts_with("tmpfs "))
            }),
            "run this test with a private /home tmpfs, never on the production host"
        );
        let home = format!("/home/easymanage-ai-test-{}", uuid::Uuid::new_v4());
        let directory = std::path::Path::new(&home).join(".npm-global/bin");
        fs::create_dir_all(&directory).unwrap();
        let binary = directory.join("codex");
        fs::write(&binary, b"fixture, never execute").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let other_uid = if super::effective_uid() == Some(2000) {
            2001
        } else {
            2000
        };
        let account = format!("fixture:x:{other_uid}:{other_uid}::{home}:/bin/bash");
        let inventory = super::ExtendedInventory::from_sources(
            Ok(account),
            Ok(String::new()),
            Ok(String::new()),
            Ok("[]".to_owned()),
            Ok("[]".to_owned()),
            Ok("[]".to_owned()),
            Ok(String::new()),
            Ok(String::new()),
            Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
            true,
            true,
        );
        fs::remove_file(&binary).unwrap();
        fs::remove_dir(&directory).unwrap();
        fs::remove_dir(std::path::Path::new(&home).join(".npm-global")).unwrap();
        fs::remove_dir(&home).unwrap();
        let codex = inventory
            .ai_tools
            .iter()
            .find(|tool| tool.name == "codex")
            .unwrap();
        assert!(
            codex
                .executable_paths
                .contains(&binary.to_string_lossy().into_owned())
        );
        assert_ne!(
            inventory.privilege.uid, other_uid,
            "fixture must represent another account"
        );
    }

    #[test]
    fn disabled_user_status_keeps_only_shared_account_inputs() {
        let inventory = super::ExtendedInventory::from_sources(
            Ok("root:x:0:0::/root:/bin/sh\nfixture:x:1000:1000::/home/fixture:/bin/sh".into()),
            Ok("fixture pts/0 2026-01-01".into()),
            Ok("3 1000 fixture - -".into()),
            Ok("[]".into()),
            Ok("[]".into()),
            Ok("[]".into()),
            Ok(String::new()),
            Ok(String::new()),
            Err(std::io::ErrorKind::NotFound.into()),
            false,
            true,
        );
        assert!(
            inventory
                .users
                .iter()
                .any(|user| user.uid == 1000 && user.home == "/home/fixture")
        );
        for user in &inventory.users {
            assert!(user.groups.is_empty() && user.session_sources.is_empty());
            assert!(user.sessions.is_none() && user.session_evidence.is_none());
            assert!(user.process_count.is_none() && user.account_status.is_none());
        }
        assert!(
            !inventory
                .issues
                .iter()
                .any(|item| matches!(item.component.as_str(), "users" | "sessions"))
        );
    }

    #[test]
    fn disabled_proxy_inventory_retains_other_inventory_without_proxy_notes() {
        let inventory = super::ExtendedInventory::from_sources(
            Ok("root:x:0:0::/root:/bin/sh\nfixture:x:1000:1000::/home/fixture:/bin/sh".into()),
            Ok(String::new()),
            Ok(String::new()),
            Ok("[]".into()),
            Ok("[]".into()),
            Ok("[]".into()),
            Ok(String::new()),
            Ok(String::new()),
            Err(std::io::ErrorKind::NotFound.into()),
            true,
            false,
        );
        assert!(inventory.proxy_services.is_empty());
        assert!(
            !inventory
                .issues
                .iter()
                .any(|issue| issue.component == "proxy")
        );
        assert!(
            inventory
                .users
                .iter()
                .any(|user| user.username == "fixture")
        );
        assert!(!inventory.ai_tools.is_empty());
    }

    #[test]
    fn combines_terminal_and_noninteractive_session_evidence_without_double_counting() {
        let terminal = parse_terminal_sessions("alice pts/0 2026-01-01\nalice pts/1 2026-01-01\n");
        assert_eq!(terminal.get("alice"), Some(&2));
        let logind =
            parse_logind_sessions(" 3 1000 alice - -\n 8 1000 alice - -\n2 sessions listed.\n")
                .expect("valid logind list");
        assert_eq!(logind.get(&1000), Some(&2));
        assert!(parse_logind_sessions("unexpected output").is_none());
    }

    #[test]
    fn account_status_describes_shell_only() {
        assert_eq!(
            account_status("/usr/sbin/nologin").as_deref(),
            Some("restricted")
        );
        assert_eq!(account_status("relative-shell").as_deref(), Some("invalid"));
    }

    #[test]
    fn decodes_linux_socket_addresses() {
        if cfg!(target_endian = "little") {
            assert_eq!(
                socket_address("0100007F:1ED2", false).as_deref(),
                Some("127.0.0.1:7890")
            );
            assert_eq!(
                socket_address("00000000000000000000000001000000:0016", true).as_deref(),
                Some("[::1]:22")
            );
        }
        assert_eq!(socket_address("invalid", false), None);
    }

    #[test]
    fn preserves_vlan_and_overlay_information() {
        let data = r#"[{"ifname":"eth0.20","operstate":"UP","mtu":1500,"linkinfo":{"info_kind":"vlan","info_data":{"id":20}},"addr_info":[{"local":"192.0.2.2","prefixlen":24}]}]"#;
        let interfaces = parse_interfaces(data).expect("valid ip JSON");
        assert_eq!(interfaces[0].vlan_id, Some(20));
        assert_eq!(interfaces[0].addresses, ["192.0.2.2/24"]);
        assert_eq!(interface_kind("zt12345", "tun"), "zerotier");
        assert_eq!(
            super::classify_interface_kind("wlp2s0", "", true, true),
            "wifi"
        );
        assert_eq!(
            super::classify_interface_kind("enp3s0", "", false, true),
            "ethernet"
        );
        let routes = parse_routes(
            r#"[{"dst":"default","dev":"eth0","gateway":"2001:db8::1"}]"#,
            true,
        )
        .expect("valid route");
        assert_eq!(routes[0].destination, "::/0");
    }
}
