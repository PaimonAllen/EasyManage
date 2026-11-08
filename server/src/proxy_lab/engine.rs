//! Disposable Mihomo process. Never reload the user's proxy or create TUN/routes.
use super::{
    history::Observation,
    vault::{Vault, write_private},
};
use anyhow::{Result, bail};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::os::unix::fs::MetadataExt;
use std::{
    collections::HashMap,
    fs,
    net::{IpAddr, TcpListener},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};
use uuid::Uuid;

#[derive(Clone, Copy, serde::Serialize)]
pub struct TestSite {
    pub name: &'static str,
    pub url: &'static str,
    pub expected: &'static str,
}
// Policy 3 remains available for historical evidence.
pub const RESTRICTED_TEST_SITES: [TestSite; 6] = [
    TestSite {
        name: "Google",
        url: "https://www.google.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "YouTube",
        url: "https://www.youtube.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Google Scholar",
        url: "https://scholar.google.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Facebook",
        url: "https://www.facebook.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Wikipedia",
        url: "https://en.wikipedia.org/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Reddit",
        url: "https://www.reddit.com/robots.txt",
        expected: "200",
    },
];
pub const RESTRICTED_FALLBACK_SITES: [TestSite; 6] = [
    TestSite {
        name: "Google 香港",
        url: "https://www.google.com.hk/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "YouTube 隐私增强域名",
        url: "https://www.youtube-nocookie.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Google Scholar 英国",
        url: "https://scholar.google.co.uk/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Facebook 移动版",
        url: "https://m.facebook.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Wikipedia 中文",
        url: "https://zh.wikipedia.org/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Reddit 经典版",
        url: "https://old.reddit.com/robots.txt",
        expected: "200",
    },
];
pub const LEGACY_TEST_SITES: [TestSite; 6] = [
    TestSite {
        name: "Google",
        url: "https://www.google.com/generate_204",
        expected: "204",
    },
    TestSite {
        name: "Bing",
        url: "https://www.bing.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Apple",
        url: "https://www.apple.com/library/test/success.html",
        expected: "200",
    },
    TestSite {
        name: "Cloudflare",
        url: "https://cp.cloudflare.com/generate_204",
        expected: "204",
    },
    TestSite {
        name: "GitHub",
        url: "https://github.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Mozilla",
        url: "https://www.mozilla.org/robots.txt",
        expected: "200",
    },
];
// Policy 4 includes independent mainstream providers, not only restricted sites.
pub const TEST_SITES: [TestSite; 10] = [
    TestSite {
        name: "Google",
        url: "https://www.google.com/generate_204",
        expected: "204",
    },
    TestSite {
        name: "YouTube",
        url: "https://www.youtube.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Cloudflare",
        url: "https://cp.cloudflare.com/generate_204",
        expected: "204",
    },
    TestSite {
        name: "Microsoft",
        url: "https://www.microsoft.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Apple",
        url: "https://www.apple.com/library/test/success.html",
        expected: "200",
    },
    TestSite {
        name: "Bing",
        url: "https://www.bing.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "GitHub",
        url: "https://github.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Mozilla",
        url: "https://detectportal.firefox.com/success.txt",
        expected: "200",
    },
    TestSite {
        name: "Wikipedia",
        url: "https://en.wikipedia.org/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Amazon",
        url: "https://www.amazon.com/robots.txt",
        expected: "200",
    },
];
pub const FALLBACK_SITES: [TestSite; 10] = [
    TestSite {
        name: "Google 香港",
        url: "https://www.google.com.hk/generate_204",
        expected: "204",
    },
    TestSite {
        name: "YouTube 隐私增强域名",
        url: "https://www.youtube-nocookie.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Cloudflare",
        url: "https://www.cloudflare.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Microsoft Learn",
        url: "https://learn.microsoft.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Apple",
        url: "https://www.apple.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Bing",
        url: "https://cn.bing.com/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "GitHub API",
        url: "https://api.github.com/",
        expected: "200",
    },
    TestSite {
        name: "Mozilla",
        url: "https://www.mozilla.org/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Wikipedia 中文",
        url: "https://zh.wikipedia.org/robots.txt",
        expected: "200",
    },
    TestSite {
        name: "Amazon 日本",
        url: "https://www.amazon.co.jp/robots.txt",
        expected: "200",
    },
];
pub fn passes(success: usize, total: usize) -> bool {
    total > 0 && success * 100 >= total * 80
}
/// One fleet-wide round; bounded per-node work stays within the 180-second clock.
pub fn availability_budget(_nodes: usize) -> Duration {
    Duration::from_secs(170)
}
fn summarize(id: Uuid, sites: Vec<i32>) -> Observation {
    let mut successes: Vec<_> = sites.iter().copied().filter(|ms| *ms >= 0).collect();
    let unknown = sites.iter().filter(|ms| **ms == -2).count();
    let pass = passes(successes.len(), sites.len());
    let latency_ms = if pass {
        successes.sort_unstable();
        let mid = successes.len() / 2;
        Some(if successes.len() % 2 == 0 {
            (successes[mid - 1] as u64 + successes[mid] as u64) / 2
        } else {
            successes[mid] as u64
        })
    } else {
        None
    };
    let state = if pass {
        if latency_ms.unwrap() > 1500 {
            "slow"
        } else {
            "up"
        }
    } else if passes(successes.len() + unknown, sites.len()) {
        "unknown"
    } else {
        "down"
    };
    Observation {
        id,
        state: state.into(),
        latency_ms,
        http_status: None,
        http_fallback: None,
        policy: super::history::POLICY,
        sites,
        link: None,
        site_feedback: vec![],
        detail: if unknown > 0 {
            "部分站点证据未确认；不把检测内核异常当作网站故障".into()
        } else {
            String::new()
        },
    }
}
fn verified_delay(evidence: Option<&Value>, url: &str, measured: i32) -> i32 {
    let Some(site) = evidence
        .and_then(|e| e.get("extra"))
        .and_then(|e| e.get(url))
    else {
        return -2;
    };
    let delay = site["history"]
        .as_array()
        .and_then(|h| h.last())
        .and_then(|v| v["delay"].as_i64());
    match (site["alive"].as_bool(), delay) {
        (Some(true), Some(ms)) if ms == measured as i64 => measured,
        (Some(false), Some(0)) => -1,
        _ => -2,
    }
}
pub struct Engine {
    pub api: reqwest::Client,
    pub proxy: Option<reqwest::Proxy>,
    child: Child,
    directory: PathBuf,
    secret: String,
    rejected: HashMap<Uuid, (String, String)>,
    verification_proxies: HashMap<Uuid, reqwest::Proxy>,
}
pub fn binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("EASYMANAGE_PROXY_CORE_BINARY") {
        let p = PathBuf::from(p);
        return trusted_binary(&p).then_some(p);
    }
    let mut paths = vec![
        PathBuf::from("/usr/local/bin/mihomo"),
        PathBuf::from("/usr/bin/mihomo"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        paths.insert(0, PathBuf::from(home).join(".easymanage/bin/mihomo-probe"));
    }
    paths.into_iter().find(|p| trusted_binary(p))
}
fn trusted_binary(path: &std::path::Path) -> bool {
    path.is_absolute()
        && fs::symlink_metadata(path).is_ok_and(|m| {
            m.is_file()
                && m.mode() & 0o022 == 0
                && m.mode() & 0o111 != 0
                && (m.uid() == 0 || fs::metadata("/proc/self").is_ok_and(|s| s.uid() == m.uid()))
        })
}
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let a = v.octets();
            !v.is_private()
                && !v.is_loopback()
                && !v.is_link_local()
                && !v.is_unspecified()
                && !v.is_broadcast()
                && !v.is_multicast()
                && !v.is_documentation()
                && a[0] != 0
                && a[0] < 224
                && !(a[0] == 100 && (64..128).contains(&a[1]))
                && !(a[0] == 198 && (a[1] == 18 || a[1] == 19))
                && !(a[0] == 192 && a[1] == 0 && a[2] == 0)
        }
        IpAddr::V6(v) => {
            if let Some(v) = v.to_ipv4_mapped() {
                return public_ip(v.into());
            }
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && s[1] == 0xdb8)
        }
    }
}
pub struct Prepared {
    proxies: Vec<Value>,
    dns_notes: HashMap<Uuid, String>,
    rejected: HashMap<Uuid, (String, String)>,
    targets: HashMap<Uuid, (std::net::SocketAddr, bool)>,
}
pub async fn prepare(nodes: &[(Uuid, Value)]) -> Prepared {
    let hosts: std::collections::HashSet<_> = nodes
        .iter()
        .filter_map(|(_, d)| d["server"].as_str().map(str::to_owned))
        .collect();
    let resolved: HashMap<_, _> =
        futures_util::future::join_all(hosts.into_iter().map(|host| async move {
            let result = super::dns::resolve(&host).await;
            (host, result)
        }))
        .await
        .into_iter()
        .collect();
    let mut result = Prepared {
        proxies: vec![],
        dns_notes: HashMap::new(),
        rejected: HashMap::new(),
        targets: HashMap::new(),
    };
    for (id, value) in nodes {
        let mut definition = value.clone();
        let host = definition["server"].as_str().unwrap_or("").to_owned();
        match resolved.get(&host) {
            Some(Ok(resolution)) => {
                let ip = resolution.ip;
                if let Some(note) = &resolution.note {
                    result.dns_notes.insert(*id, note.clone());
                }
                let protocol = definition["type"].as_str().unwrap_or("");
                let udp = matches!(protocol, "hysteria" | "hysteria2" | "tuic" | "wireguard");
                let key = if matches!(protocol, "hysteria" | "hysteria2" | "tuic") {
                    "sni"
                } else {
                    "servername"
                };
                if host.parse::<IpAddr>().is_err() && definition.get(key).is_none() {
                    definition[key] = host.into();
                }
                if let Some(port) = definition["port"]
                    .as_u64()
                    .filter(|p| *p > 0 && *p <= 65535)
                {
                    result
                        .targets
                        .insert(*id, (std::net::SocketAddr::new(ip, port as u16), udp));
                }
                definition["server"] = ip.to_string().into();
                definition["name"] = id.to_string().into();
                result.proxies.push(definition);
            }
            reason => {
                let blocked = reason.is_some_and(|r| r.as_ref().is_err_and(|e| e.blocked()));
                result.rejected.insert(
                    *id,
                    (
                        if blocked { "unsupported" } else { "unknown" }.into(),
                        if blocked {
                            "禁止访问本机、内网或保留地址".into()
                        } else {
                            reason
                                .and_then(|r| r.as_ref().err())
                                .map(|e| e.detail())
                                .unwrap_or_else(|| "节点 DNS 未取得地址；未开始连通性检测".into())
                        },
                    ),
                );
            }
        }
    }
    result
}
impl Prepared {
    pub fn dns_note(&self, id: Uuid) -> Option<&str> {
        self.dns_notes.get(&id).map(String::as_str)
    }
    pub fn subset(&self, ids: &std::collections::HashSet<Uuid>) -> Self {
        Self {
            dns_notes: self
                .dns_notes
                .iter()
                .filter(|(id, _)| ids.contains(id))
                .map(|(id, v)| (*id, v.clone()))
                .collect(),
            proxies: self
                .proxies
                .iter()
                .filter(|p| {
                    p["name"]
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .is_some_and(|id| ids.contains(&id))
                })
                .cloned()
                .collect(),
            rejected: self
                .rejected
                .iter()
                .filter(|(id, _)| ids.contains(id))
                .map(|(id, v)| (*id, v.clone()))
                .collect(),
            targets: self
                .targets
                .iter()
                .filter(|(id, _)| ids.contains(id))
                .map(|(id, v)| (*id, *v))
                .collect(),
        }
    }
    pub fn link(
        &self,
        id: Uuid,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = super::history::NodeLink> + Send + 'static>,
    > {
        let target = self.targets.get(&id).copied();
        let rejected = self.rejected.get(&id).cloned();
        let dns_note = self.dns_notes.get(&id).cloned();
        Box::pin(async move {
            use super::history::NodeLink;
            if let Some((state, detail)) = rejected {
                return NodeLink {
                    state,
                    latency_ms: None,
                    detail,
                    kind: "tcp".into(),
                };
            }
            let Some((address, udp)) = target else {
                return NodeLink {
                    state: "unknown".into(),
                    latency_ms: None,
                    detail: "没有可确认的节点端口".into(),
                    kind: "tcp".into(),
                };
            };
            if udp {
                return NodeLink {
                    state: "unsupported".into(),
                    latency_ms: None,
                    detail: "此节点使用 UDP；不以 TCP 失败推断 UDP 不可用".into(),
                    kind: "udp".into(),
                };
            }
            let mut value = link_address(address).await;
            if let Some(note) = dns_note {
                value.detail.push_str("；");
                value.detail.push_str(&note);
            }
            value
        })
    }
}
async fn link_address(address: std::net::SocketAddr) -> super::history::NodeLink {
    let counters =
        easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::TcpProbe);
    let mut attempt = counters.begin();
    let start = tokio::time::Instant::now();
    let connected = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(address),
    )
    .await;
    let success = matches!(&connected, Ok(Ok(_)));
    let local_failure =
        matches!(&connected,Ok(Err(error)) if matches!(error.raw_os_error(),Some(12|23|24|105)));
    attempt.finish(success);
    super::history::NodeLink {
        state: if success {
            if start.elapsed().as_millis() > 1500 {
                "slow"
            } else {
                "up"
            }
        } else if local_failure {
            "unknown"
        } else {
            "down"
        }
        .into(),
        latency_ms: success.then(|| start.elapsed().as_millis().max(1) as u64),
        detail: if success {
            "平台直接连接节点端口成功；此项不经过代理访问网站"
        } else if local_failure {
            "平台本地连接资源不足；不能据此判断节点故障"
        } else {
            "平台直接连接节点端口失败或 5 秒超时"
        }
        .into(),
        kind: "tcp".into(),
    }
}
impl Engine {
    pub async fn start(vault: &Vault, nodes: &[(Uuid, Value)], scoring: bool) -> Result<Self> {
        let prepared = prepare(nodes).await;
        Self::from_prepared(vault, &prepared, scoring).await
    }
    pub async fn from_prepared(vault: &Vault, prepared: &Prepared, scoring: bool) -> Result<Self> {
        let executable = binary().ok_or_else(|| anyhow::anyhow!("Mihomo 检测内核未配置"))?;
        let directory = vault.workspace()?;
        let result = Self::launch(executable, directory.clone(), prepared, scoring).await;
        if result.is_err() {
            let _ = fs::remove_dir_all(directory);
        }
        result
    }
    async fn launch(
        executable: PathBuf,
        directory: PathBuf,
        prepared: &Prepared,
        scoring: bool,
    ) -> Result<Self> {
        let proxies = &prepared.proxies;
        let rejected = prepared.rejected.clone();
        let socket = directory.join("c.sock");
        let secret = Uuid::new_v4().simple().to_string();
        let mut config = json!({"allow-lan":false,"bind-address":"127.0.0.1","mode":"rule","log-level":"silent","ipv6":true,
            "external-controller-unix":socket,"secret":secret,"find-process-mode":"off","geo-auto-update":false,"unified-delay":false,
            "dns":{"enable":false},"tun":{"enable":false},"sniffer":{"enable":false},
            "profile":{"store-selected":false,"store-fake-ip":false},"proxies":proxies,
            "proxy-groups":[{"name":"PROXY_LAB","type":"select","proxies":proxies.iter().filter_map(|p|p["name"].as_str()).collect::<Vec<_>>()}],
            "rules":["MATCH,PROXY_LAB"]});
        // An empty group uses REJECT; it must never fall back to DIRECT.
        if proxies.is_empty() {
            config["proxy-groups"][0]["proxies"] = json!(["REJECT"]);
        }
        let mut verification_proxies = HashMap::new();
        let mut proxy_port = None;
        let proxy = if scoring || !proxies.is_empty() {
            let reserved = TcpListener::bind(("127.0.0.1", 0))?;
            let port = reserved.local_addr()?.port();
            proxy_port = Some(port);
            let password = Uuid::new_v4().simple().to_string();
            config["mixed-port"] = port.into();
            if scoring {
                config["authentication"] = json!([format!("probe:{password}")]);
            } else {
                let mut authentication = Vec::new();
                let mut rules = Vec::new();
                for definition in proxies {
                    let Some(name) = definition["name"].as_str() else {
                        continue;
                    };
                    let id = Uuid::parse_str(name)?;
                    authentication.push(format!("{name}:{password}"));
                    rules.push(format!("IN-USER,{name},{name}"));
                    verification_proxies.insert(
                        id,
                        reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))?
                            .basic_auth(name, &password),
                    );
                }
                rules.push("MATCH,PROXY_LAB".into());
                config["authentication"] = json!(authentication);
                config["rules"] = json!(rules);
            }
            drop(reserved);
            Some(
                reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))?
                    .basic_auth("probe", &password),
            )
        } else {
            None
        };
        let path = directory.join("config.json");
        write_private(&path, &serde_json::to_vec(&config)?)?;
        let mut command = Command::new("/usr/bin/timeout");
        let watchdog = if scoring {
            "660".to_owned()
        } else {
            (availability_budget(proxies.len()).as_secs() + 5).to_string()
        };
        command
            .args(["--signal=TERM", "--kill-after=2", &watchdog])
            .arg(executable)
            .arg("-d")
            .arg(&directory)
            .arg("-f")
            .arg(path)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &directory)
            .env("GOMAXPROCS", if scoring { "1" } else { "8" })
            .env("GOMEMLIMIT", if scoring { "48MiB" } else { "512MiB" })
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(false);
        let api = reqwest::Client::builder()
            .unix_socket(socket.as_path())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()?;
        let child = command.spawn()?;
        let mut engine = Self {
            api,
            proxy,
            child,
            directory,
            secret,
            rejected,
            verification_proxies,
        };
        for _ in 0..40 {
            if engine.child.try_wait()?.is_some() {
                bail!("检测内核启动失败，未输出配置或凭据");
            }
            let controller_ready = engine
                .api
                .get("http://localhost/version")
                .timeout(Duration::from_millis(200))
                .bearer_auth(&engine.secret)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if controller_ready {
                let listener_ready = if let Some(port) = proxy_port {
                    matches!(
                        tokio::time::timeout(
                            Duration::from_millis(200),
                            tokio::net::TcpStream::connect(("127.0.0.1", port))
                        )
                        .await,
                        Ok(Ok(_))
                    )
                } else {
                    true
                };
                if listener_ready {
                    return Ok(engine);
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("检测内核未就绪，未输出配置或凭据")
    }
    #[cfg(test)]
    pub fn check(
        &self,
        id: Uuid,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Observation> + Send + 'static>> {
        self.check_with_feedback(id, vec![])
    }
    pub fn check_with_feedback(
        &self,
        id: Uuid,
        prior: Vec<super::history::SiteFeedback>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Observation> + Send + 'static>> {
        let verification_proxy = self.verification_proxies.get(&id).cloned();
        let api = self.api.clone();
        let secret = self.secret.clone();
        let rejected = self.rejected.get(&id).cloned();
        Box::pin(async move {
            if let Some((state, reason)) = rejected.as_ref() {
                return Observation {
                    id,
                    state: state.clone(),
                    latency_ms: None,
                    http_status: None,
                    http_fallback: None,
                    detail: reason.clone(),
                    policy: super::history::POLICY,
                    sites: vec![],
                    link: None,
                    site_feedback: vec![],
                };
            }
            let cooldowns = prior.clone();
            let results = stream::iter(TEST_SITES.into_iter().enumerate().map(|(index, site)| {
                let api = api.clone();
                let secret = secret.clone();
                let cooling = cooldowns.iter().any(|f| {
                    f.site == index && f.retry_at.is_some_and(|at| at > chrono::Utc::now())
                });
                async move {
                    if cooling {
                        return (index, -2, None);
                    }
                    // Spread a node's requests too; all futures remain asynchronous.
                    tokio::time::sleep(Duration::from_millis(index as u64 * 100)).await;
                    let counters = easymanage_runtime::network::counter(
                        easymanage_protocol::NetworkGroup::HttpProbe,
                    );
                    let mut attempt = counters.begin();
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
                    let mut backoff = 10u64;
                    let response = loop {
                        let response = api
                            .get(format!("http://localhost/proxies/{id}/delay"))
                            .bearer_auth(&secret)
                            .query(&[
                                ("url", site.url),
                                ("timeout", "8000"),
                                ("expected", site.expected),
                            ])
                            .send()
                            .await;
                        // A failed Unix connection has not delivered an HTTP request
                        // to the core, hence has not started any external site probe.
                        // Retry only that local pre-connect failure, never a response,
                        // body error or timeout after delivery. No probe concurrency cap.
                        if response.as_ref().is_err_and(|e| e.is_connect())
                            && tokio::time::Instant::now() + Duration::from_millis(backoff)
                                < deadline
                        {
                            let jitter = (id.as_u128() as u64).wrapping_add(index as u64 * 13) % 23;
                            tokio::time::sleep(Duration::from_millis(backoff + jitter)).await;
                            backoff = (backoff * 2).min(200);
                            continue;
                        }
                        break response;
                    };
                    let (value, reason) = match response {
                        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                            Ok(v) => match v["delay"].as_u64() {
                                Some(ms) if ms > 0 && ms <= 8000 => (ms as i32, None),
                                _ => (-2, Some("本地检测接口返回的耗时无效".to_owned())),
                            },
                            Err(_) => (-2, Some("本地检测接口返回了无效结果".to_owned())),
                        },
                        Ok(r) if matches!(r.status().as_u16(), 503 | 504) => (-1, None),
                        Ok(r) => (
                            -2,
                            Some(format!("本地检测接口 HTTP {}", r.status().as_u16())),
                        ),
                        Err(e) if e.is_connect() => {
                            (-2, Some("平台与检测内核的本地连接失败".to_owned()))
                        }
                        Err(e) if e.is_timeout() => (-2, Some("本地检测接口等待超时".to_owned())),
                        Err(_) => (-2, Some("本地检测接口请求失败".to_owned())),
                    };
                    attempt.finish(value >= 0);
                    (index, value, reason)
                }
            }))
            .buffer_unordered(3)
            .collect::<Vec<_>>()
            .await;
            let mut sites = vec![-2; TEST_SITES.len()];
            let mut reasons = std::collections::BTreeSet::new();
            for (index, value, reason) in results {
                sites[index] = value;
                if let Some(reason) = reason {
                    reasons.insert(reason);
                }
            }
            // Mihomo v1.19.33's /delay can return a latency for a rejected HTTP
            // status. Only its URL-specific extra.alive/history verifies expected.
            // This additional local Unix read makes no outbound website request.
            let evidence = match api
                .get(format!("http://localhost/proxies/{id}"))
                .bearer_auth(&secret)
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
                _ => None,
            };
            // An API 503/504 has no target HTTP status. Independently confirm
            // every failure/unknown through this exact node, not only wrong-status latencies.
            let mut inspect: Vec<_> = sites
                .iter()
                .enumerate()
                .filter_map(|(i, v)| (*v < 0).then_some(i))
                .collect();
            for (index, site) in TEST_SITES.iter().enumerate() {
                if sites[index] >= 0 {
                    sites[index] = verified_delay(evidence.as_ref(), site.url, sites[index]);
                    if sites[index] < 0 && !inspect.contains(&index) {
                        inspect.push(index);
                    }
                    if sites[index] == -2 {
                        reasons.insert("缺少网站 HTTP 状态的核验记录".to_owned());
                    }
                }
            }
            for feedback in &prior {
                if feedback.site < TEST_SITES.len()
                    && feedback.retry_at.is_some_and(|at| at > chrono::Utc::now())
                    && !inspect.contains(&feedback.site)
                {
                    inspect.push(feedback.site);
                }
            }
            // Controller failure alone is never outage evidence. If native
            // verification cannot run, retain unknown instead of an unverified red.
            for index in &inspect {
                sites[*index] = -2;
            }
            let mut feedback = Vec::new();
            if let Some(proxy) = verification_proxy.filter(|_| !inspect.is_empty()) {
                if let Ok(client) = verification_client(proxy) {
                    let tasks = inspect.into_iter().map(|index| {
                        let client = client.clone();
                        let previous = prior.iter().find(|f| f.site == index).cloned();
                        async move {
                            (
                                index,
                                super::fallback::recover(
                                    &client,
                                    index,
                                    TEST_SITES[index],
                                    FALLBACK_SITES[index],
                                    previous,
                                )
                                .await,
                            )
                        }
                    });
                    for (index, result) in stream::iter(tasks)
                        .buffer_unordered(3)
                        .collect::<Vec<_>>()
                        .await
                    {
                        if let Some((value, info)) = result {
                            sites[index] = value;
                            feedback.push(info);
                        }
                    }
                }
            }
            let mut observation = summarize(id, sites);
            for previous in prior {
                if previous
                    .fallback_retry_at
                    .is_some_and(|at| at > chrono::Utc::now())
                    && !feedback.iter().any(|f| f.site == previous.site)
                {
                    let mut previous = previous;
                    if observation
                        .sites
                        .get(previous.site)
                        .is_some_and(|value| *value >= 0)
                    {
                        previous.primary_status = TEST_SITES[previous.site].expected.parse().ok();
                        previous.retry_at = None;
                        previous.rate_limited = false;
                        previous.outcome = "primary_ok".into();
                    }
                    feedback.push(previous);
                }
            }
            let has_limits = feedback
                .iter()
                .any(|f| f.rate_limited && f.outcome != "fallback_ok");
            observation.site_feedback = feedback;
            if has_limits {
                reasons.insert("部分网站限流或处于冷却期；回退明细见站点结果".into());
            }
            if !reasons.is_empty() {
                observation.detail = reasons.into_iter().collect::<Vec<_>>().join("；");
            }
            observation
        })
    }
    pub async fn score_client(&self, id: Uuid) -> Result<reqwest::Client> {
        if self.rejected.contains_key(&id) {
            bail!("节点地址不可用于公网出口检测");
        }
        let response = self
            .api
            .put("http://localhost/proxies/PROXY_LAB")
            .bearer_auth(&self.secret)
            .json(&json!({"name":id.to_string()}))
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("无法选择独立检测出口");
        }
        Ok(reqwest::Client::builder()
            .no_proxy()
            .proxy(
                self.proxy
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("no score proxy"))?,
            )
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(12))
            .pool_max_idle_per_host(0)
            .user_agent("EasyManage-ProxyLab/0.5 (manual IP reputation check)")
            .build()?)
    }
    fn terminate(&mut self) {
        if let Some(id) = self.child.id() {
            // timeout and Mihomo share this dedicated process group. TERM lets the
            // watchdog forward and reap; killing only the wrapper would orphan the core.
            let _ = std::process::Command::new("/bin/kill")
                .args(["-TERM", "--", &format!("-{id}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    pub async fn shutdown(&mut self) {
        self.terminate();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
    }
}
fn verification_client(proxy: reqwest::Proxy) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .proxy(proxy)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .pool_max_idle_per_host(0)
        .user_agent("EasyManage-ProxyLab/0.5 (availability headers)")
        .build()?)
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.terminate();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn real_core_header_checks_are_authenticated_and_bound_to_each_node() {
        if std::env::var_os("EASYMANAGE_PROXY_CORE_TEST").is_none() {
            return;
        }
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn headers(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
            let mut bytes = Vec::new();
            let mut chunk = [0; 1024];
            while !bytes.ends_with(b"\r\n\r\n") {
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0 && bytes.len() < 16384);
                bytes.extend_from_slice(&chunk[..n]);
            }
            bytes
        }
        let directory = std::env::temp_dir().join(format!("proxy-user-route-{}", Uuid::new_v4()));
        let vault = Vault {
            directory: directory.clone(),
        };
        vault.initialize(true).unwrap();
        let mut proxies = Vec::new();
        let mut fixtures = Vec::new();
        let mut ids = Vec::new();
        for status in [429, 200] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let id = Uuid::new_v4();
            ids.push(id);
            proxies.push(
                json!({"name":id.to_string(),"type":"http","server":"127.0.0.1","port":port}),
            );
            fixtures.push(tokio::spawn(async move {
                let (mut socket,_)=listener.accept().await.unwrap();let request=headers(&mut socket).await;
                assert!(request.starts_with(b"CONNECT header-fixture.invalid:80 "));
                socket.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
                let request=headers(&mut socket).await;assert!(request.starts_with(b"HEAD /"));
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nRetry-After: 600\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            }));
        }
        // Fixture-only localhost upstreams: the production resolver rejects these.
        let prepared = Prepared {
            dns_notes: HashMap::new(),
            proxies,
            rejected: HashMap::new(),
            targets: HashMap::new(),
        };
        let mut engine = Engine::from_prepared(&vault, &prepared, false)
            .await
            .unwrap();
        let tasks = ids.iter().map(|id| async {
            let client = reqwest::Client::builder()
                .no_proxy()
                .proxy(engine.verification_proxies[id].clone())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            client
                .head("http://header-fixture.invalid/")
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        });
        assert_eq!(futures_util::future::join_all(tasks).await, vec![429, 200]);
        for task in fixtures {
            task.await.unwrap();
        }
        let secured = verification_client(engine.verification_proxies[&ids[0]].clone()).unwrap();
        assert!(
            secured
                .head("http://header-fixture.invalid/")
                .send()
                .await
                .is_err()
        );
        engine.shutdown().await;
        drop(engine);
        fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn https_connect_is_bound_to_the_requested_proxy_and_never_direct() {
        if std::env::var_os("EASYMANAGE_PROXY_CORE_TEST").is_none() {
            return;
        }
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let directory =
            std::env::temp_dir().join(format!("proxy-connect-route-{}", Uuid::new_v4()));
        let vault = Vault {
            directory: directory.clone(),
        };
        vault.initialize(true).unwrap();
        let mut proxies = Vec::new();
        let mut ids = Vec::new();
        let mut upstreams = Vec::new();
        for index in 0..2 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let id = Uuid::new_v4();
            ids.push(id);
            proxies.push(json!({"name":id.to_string(),"type":"http","server":"127.0.0.1","port":listener.local_addr().unwrap().port()}));
            upstreams.push(tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                    request.push(byte[0]);
                    assert!(request.len() < 16384);
                }
                assert!(request.starts_with(b"CONNECT tls-route-fixture.invalid:443 "));
                socket
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await
                    .unwrap();
                let mut first = [0];
                assert_eq!(socket.read(&mut first).await.unwrap(), 1);
                assert_eq!(
                    first[0], 0x16,
                    "the selected upstream must receive the real TLS handshake"
                );
                index
            }));
        }
        let prepared = Prepared {
            dns_notes: HashMap::new(),
            proxies,
            rejected: HashMap::new(),
            targets: HashMap::new(),
        };
        let mut core = Engine::from_prepared(&vault, &prepared, false)
            .await
            .unwrap();
        let checks = ids.iter().map(|id| {
            let client = verification_client(core.verification_proxies[id].clone()).unwrap();
            async move {
                assert!(
                    client
                        .head("https://tls-route-fixture.invalid/")
                        .send()
                        .await
                        .is_err()
                );
            }
        });
        futures_util::future::join_all(checks).await;
        let seen = tokio::time::timeout(
            Duration::from_secs(4),
            futures_util::future::join_all(upstreams),
        )
        .await
        .unwrap();
        assert_eq!(
            seen.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![0, 1]
        );
        core.shutdown().await;
        drop(core);
        fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn node_link_is_a_real_independent_connection_and_udp_is_not_fake_down() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let up = link_address(address).await;
        assert_eq!(up.state, "up");
        assert!(up.latency_ms.is_some());
        let accepted = listener.accept().await.unwrap();
        drop(accepted);
        drop(listener);
        assert_eq!(link_address(address).await.state, "down");
        let id = Uuid::new_v4();
        let prepared = Prepared {
            dns_notes: HashMap::new(),
            proxies: vec![],
            rejected: HashMap::new(),
            targets: HashMap::from([(id, (address, true))]),
        };
        assert_eq!(prepared.link(id).await.state, "unsupported");
        let bad = prepare(&[(id, json!({"type":"http","server":"127.0.0.1","port":443}))]).await;
        assert_eq!(bad.link(id).await.state, "unsupported");
        assert!(bad.proxies.is_empty());
    }
    #[test]
    fn inclusive_eighty_percent_preserves_unknown_evidence() {
        assert!(passes(5, 6));
        assert!(passes(4, 5));
        assert!(passes(8, 10));
        assert!(!passes(7, 10));
        assert_eq!(
            summarize(Uuid::new_v4(), vec![17, 17, 17, 17, 17, 17, 17, 17, -1, -1]).state,
            "up"
        );
        assert_eq!(
            summarize(Uuid::new_v4(), vec![17, 17, 17, 17, 17, 17, 17, -1, -1, -1]).state,
            "down"
        );
        assert_eq!(
            summarize(Uuid::new_v4(), vec![17, 17, 17, 17, 17, 17, 17, -2, -1, -1]).state,
            "unknown"
        );
        assert!(!passes(0, 0));
        let id = Uuid::new_v4();
        assert_eq!(summarize(id, vec![17, 18, 19, 20, 5000, -1]).state, "up");
        assert_eq!(
            summarize(id, vec![1501, 1600, 1700, 1800, 5000, -1]).state,
            "slow"
        );
        assert_eq!(summarize(id, vec![17, 18, 19, 20, -1, -1]).state, "down");
        assert_eq!(summarize(id, vec![17, 18, 19, 20, -1, -2]).state, "unknown");
        assert_eq!(summarize(id, vec![17, 18, 19, 20, 21, -2]).state, "up");
        assert_eq!(summarize(id, vec![-1, -1, -2, -2, -2, -2]).state, "down");
    }
    #[test]
    fn delay_alone_does_not_verify_http_success() {
        let url = TEST_SITES[0].url;
        let good = json!({"extra":{url:{"alive":true,"history":[{"delay":17}]}}});
        let blocked = json!({"extra":{url:{"alive":false,"history":[{"delay":0}]}}});
        assert_eq!(verified_delay(Some(&good), url, 17), 17);
        assert_eq!(verified_delay(Some(&blocked), url, 17), -1);
        assert_eq!(verified_delay(Some(&good), url, 18), -2);
        assert_eq!(verified_delay(None, url, 17), -2);
        assert!(availability_budget(2048).as_secs() < 180);
    }
    #[test]
    fn refuses_internal_and_special_destinations() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "100.64.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
            "2001:db8::1",
        ] {
            assert!(!public_ip(address.parse().unwrap()), "{address}");
        }
        for address in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_ip(address.parse().unwrap()));
        }
    }
    #[tokio::test]
    async fn real_core_parallel_unreachable_nodes_keep_network_failures_distinct_from_controller_errors()
     {
        if std::env::var_os("EASYMANAGE_PROXY_CORE_TEST").is_none() {
            return;
        }
        let directory = std::env::temp_dir().join(format!("proxy-burst-{}", Uuid::new_v4()));
        let vault = Vault {
            directory: directory.clone(),
        };
        vault.initialize(true).unwrap();
        let nodes: Vec<_> = (0..367)
            .map(|_| {
                (
                    Uuid::new_v4(),
                    json!({"type":"http","server":"8.8.8.8","port":443}),
                )
            })
            .collect();
        let mut engine = Engine::start(&vault, &nodes, false).await.unwrap();
        let values =
            futures_util::future::join_all(nodes.iter().map(|(id, _)| engine.check(*id))).await;
        let mut states = std::collections::BTreeMap::new();
        for value in values {
            *states.entry((value.state, value.detail)).or_insert(0) += 1;
        }
        engine.shutdown().await;
        drop(engine);
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            states.keys().filter(|(state, _)| state != "down").count(),
            0,
            "{states:?}"
        );
    }
    #[tokio::test]
    async fn real_core_expected_status_requires_url_specific_evidence() {
        if std::env::var_os("EASYMANAGE_PROXY_CORE_TEST").is_none() {
            return;
        }
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // The surrounding opt-in test runner supplies a private network namespace.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let fixture = tokio::spawn(async move {
            for status in [403, 200] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 2048];
                let n = socket.read(&mut bytes).await.unwrap();
                assert!(bytes[..n].starts_with(b"HEAD / HTTP/1.1\r\n"));
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            }
        });
        let directory =
            std::env::temp_dir().join(format!("proxy-http-evidence-{}", Uuid::new_v4()));
        let vault = Vault {
            directory: directory.clone(),
        };
        vault.initialize(true).unwrap();
        let mut engine = Engine::start(&vault, &[], false).await.unwrap();
        let config_path = engine.directory.join("fixture-config.json");
        let mut config: Value =
            serde_json::from_slice(&fs::read(engine.directory.join("config.json")).unwrap())
                .unwrap();
        // Fixture-only built-in direct adapter; production imports never accept it.
        let id = Uuid::new_v4();
        config["proxies"] = json!([{"name":id.to_string(),"type":"direct"}]);
        write_private(&config_path, &serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(
            engine
                .api
                .put("http://localhost/configs")
                .bearer_auth(&engine.secret)
                .json(&json!({"path":config_path}))
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
        for expected_result in [-1, 1] {
            let response: Value = engine
                .api
                .get(format!("http://localhost/proxies/{id}/delay"))
                .bearer_auth(&engine.secret)
                .query(&[
                    ("url", url.as_str()),
                    ("timeout", "8000"),
                    ("expected", "200"),
                ])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let measured = response["delay"].as_i64().unwrap() as i32;
            let evidence: Value = engine
                .api
                .get(format!("http://localhost/proxies/{id}"))
                .bearer_auth(&engine.secret)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let verified = verified_delay(Some(&evidence), &url, measured);
            if expected_result == -1 {
                assert_eq!(verified, -1);
            } else {
                assert!(verified > 0);
            }
        }
        fixture.await.unwrap();
        engine.shutdown().await;
        drop(engine);
        fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn real_core_has_no_tun_listener_or_orphan_after_shutdown() {
        // Opt-in integration test: an explicitly bound known executable in a private namespace.
        if std::env::var_os("EASYMANAGE_PROXY_CORE_TEST").is_none() {
            return;
        }
        let directory = std::env::temp_dir().join(format!("proxy-engine-{}", Uuid::new_v4()));
        let vault = Vault {
            directory: directory.clone(),
        };
        vault.initialize(true).unwrap();
        let mut engine = Engine::start(&vault, &[], false).await.unwrap();
        let pid = engine.child.id().unwrap();
        let children = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap();
        let config: Value = engine
            .api
            .get("http://localhost/configs")
            .bearer_auth(&engine.secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(config["mixed-port"], 0);
        assert_eq!(config["tun"]["enable"], false);
        engine.shutdown().await;
        drop(engine);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        for child in children.split_whitespace() {
            assert!(
                !std::path::Path::new(&format!("/proc/{child}")).exists(),
                "orphaned probe core"
            );
        }
        let id = Uuid::new_v4();
        let node =
            serde_json::json!({"name":"fixture","type":"http","server":"8.8.8.8","port":443});
        let mut scoring = Engine::start(&vault, &[(id, node)], true).await.unwrap();
        assert!(scoring.score_client(id).await.is_ok());
        let config: Value = scoring
            .api
            .get("http://localhost/configs")
            .bearer_auth(&scoring.secret)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let port = config["mixed-port"].as_u64().unwrap();
        assert!(port > 0);
        let unauthenticated = reqwest::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let response = unauthenticated
            .get("http://fixture.invalid/")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 407);
        scoring.shutdown().await;
        drop(scoring);
        fs::remove_dir_all(directory).unwrap();
    }
}
