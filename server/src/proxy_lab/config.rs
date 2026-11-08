//! Bounded YAML analysis. No imported rules, scripts, providers or controller options execute.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;
use std::{collections::HashSet, fs, io::Read, path::PathBuf};

pub const MAX_YAML: usize = 1024 * 1024;
pub const MAX_NODES: usize = 256;
pub struct ParsedNode {
    pub identity: String,
    pub name: String,
    pub protocol: String,
    pub region: String,
    pub definition: Value,
    pub unsupported: Option<String>,
}
pub struct Analysis {
    pub nodes: Vec<ParsedNode>,
    pub warnings: Vec<String>,
}

pub fn region(name: &str) -> &'static str {
    let upper = name.to_uppercase();
    let words: Vec<_> = upper.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    for (code, marks, tokens) in [
        ("US", &['🇺', '🇸'][..], &["US", "USA", "UNITEDSTATES"][..]),
        ("JP", &['🇯', '🇵'][..], &["JP", "JPN", "JAPAN"][..]),
        ("SG", &['🇸', '🇬'][..], &["SG", "SGP", "SINGAPORE"][..]),
        (
            "TW",
            &['🇹', '🇼'][..],
            &["TW", "TWN", "TAIWAN", "TAIPEI"][..],
        ),
    ] {
        let flag: String = marks.iter().collect();
        let chinese = match code {
            "US" => name.contains("美国") || name.contains("美國"),
            "JP" => name.contains("日本") || name.contains("东京") || name.contains("大阪"),
            "SG" => name.contains("新加坡") || name.contains("狮城"),
            _ => name.contains("台湾") || name.contains("台灣") || name.contains("台北"),
        };
        if name.contains(&flag) || chinese || tokens.iter().any(|t| words.contains(t)) {
            return code;
        }
    }
    "OTHER"
}

// Reject expansion before deserialization, including recursive/billion-laughs aliases.
fn no_aliases(text: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let mut previous = ' ';
    let mut comment = false;
    let chars: Vec<_> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            comment = false;
        }
        if comment {
            continue;
        }
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && q == '"' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
        } else if c == '#' && previous.is_whitespace() {
            comment = true;
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if (c == '&' || c == '*')
            && (previous.is_whitespace() || ":,[{-".contains(previous))
            && chars
                .get(i + 1)
                .is_some_and(|n| n.is_alphanumeric() || *n == '_')
        {
            return false;
        }
        previous = c;
    }
    true
}

fn bounded(value: &serde_yaml_ng::Value, depth: usize, budget: &mut usize) -> bool {
    if depth > 32 || *budget == 0 {
        return false;
    }
    *budget -= 1;
    use serde_yaml_ng::Value as Y;
    match value {
        Y::Mapping(m) => m
            .iter()
            .all(|(k, v)| bounded(k, depth + 1, budget) && bounded(v, depth + 1, budget)),
        Y::Sequence(s) => s.iter().all(|v| bounded(v, depth + 1, budget)),
        Y::Tagged(_) => false,
        Y::String(s) => s.len() <= 64 * 1024,
        _ => true,
    }
}

pub fn parse(text: &str) -> Result<Analysis, String> {
    if text.is_empty() || text.len() > MAX_YAML {
        return Err("YAML 必须非空且不超过 1 MiB".into());
    }
    if !no_aliases(text) {
        return Err("为限制解析资源，当前不接受 YAML 锚点和别名；请上传展开后的配置".into());
    }
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(text).map_err(|e| {
        e.location().map_or_else(
            || "YAML 格式无效".into(),
            |p| format!("YAML 格式无效：第 {} 行、第 {} 列", p.line(), p.column()),
        )
    })?;
    if !bounded(&yaml, 0, &mut 100_000) {
        return Err("YAML 层级、标签或内容规模超出解析范围".into());
    }
    let value: Value = serde_json::to_value(yaml).map_err(|_| "YAML 字段必须使用字符串键")?;
    let proxies = value
        .get("proxies")
        .and_then(Value::as_array)
        .ok_or("需要包含 proxies 列表的 Clash / Mihomo YAML")?;
    if proxies.is_empty() || proxies.len() > MAX_NODES {
        return Err(format!("每份配置需包含 1–{MAX_NODES} 个节点；不会静默截断"));
    }
    let mut nodes = Vec::new();
    let mut identities = HashSet::new();
    let mut names = HashSet::new();
    for proxy in proxies {
        let name = proxy
            .get("name")
            .and_then(Value::as_str)
            .ok_or("节点缺少名称")?
            .trim();
        let protocol = proxy
            .get("type")
            .and_then(Value::as_str)
            .ok_or("节点缺少类型")?
            .to_lowercase();
        let server = proxy
            .get("server")
            .and_then(Value::as_str)
            .ok_or("节点缺少服务器地址")?;
        let port = proxy
            .get("port")
            .and_then(|p| p.as_u64().or_else(|| p.as_str()?.parse().ok()))
            .filter(|p| (1..=65535).contains(p))
            .ok_or("节点端口无效")?;
        if name.is_empty()
            || name.len() > 256
            || name.chars().any(char::is_control)
            || !names.insert(name.to_owned())
            || server.is_empty()
            || server.len() > 253
            || server
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || ".-:".contains(c)))
        {
            return Err("节点名称重复、过长或地址格式无效".into());
        }
        let identity = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!([name, protocol, server, port])).unwrap(),
        ));
        if !identities.insert(identity.clone()) {
            return Err("配置存在重复节点".into());
        }
        let supported = [
            "http",
            "socks5",
            "ss",
            "ssr",
            "vmess",
            "vless",
            "trojan",
            "hysteria",
            "hysteria2",
            "tuic",
            "snell",
            "anytls",
        ]
        .contains(&protocol.as_str());
        let chained = ["dialer-proxy", "interface-name", "routing-mark"]
            .iter()
            .any(|k| proxy.get(k).is_some());
        let mut definition = serde_json::Map::new();
        for key in [
            "name",
            "type",
            "server",
            "port",
            "cipher",
            "password",
            "uuid",
            "alterId",
            "udp",
            "tls",
            "network",
            "servername",
            "sni",
            "client-fingerprint",
            "fingerprint",
            "skip-cert-verify",
            "alpn",
            "ws-opts",
            "grpc-opts",
            "http-opts",
            "h2-opts",
            "reality-opts",
            "packet-encoding",
            "flow",
            "obfs",
            "obfs-password",
            "up",
            "down",
            "auth",
            "auth-str",
            "fast-open",
            "congestion-controller",
            "disable-sni",
            "reduce-rtt",
            "max-udp-relay-packet-size",
            "username",
            "plugin",
            "plugin-opts",
            "ip-version",
            "tfo",
            "ports",
            "udp-over-tcp",
        ] {
            if let Some(v) = proxy.get(key) {
                definition.insert(key.into(), v.clone());
            }
        }
        definition.insert("port".into(), port.into());
        definition.insert("type".into(), protocol.clone().into());
        // Proxy definitions cannot ask the core to read local certificates or keys.
        if contains_file_reference(&Value::Object(definition.clone()), "") {
            return Err("节点包含本地文件引用；请使用不依赖平台文件的独立节点配置".into());
        }
        nodes.push(ParsedNode {
            identity,
            name: name.into(),
            protocol,
            region: region(name).into(),
            definition: Value::Object(definition),
            unsupported: if !supported {
                Some("当前检测内核暂不执行此代理类型".into())
            } else if chained {
                Some("链式代理或网络绑定需独立配置，暂不执行".into())
            } else {
                None
            },
        });
    }
    let mut warnings = vec!["地区先按节点名称推断；手动评分取得出口 IP 地区后会单独标注。".into()];
    if value.get("proxy-providers").is_some() {
        warnings.push("订阅提供者不会自动下载；当前仅导入已展开的 proxies 节点。".into());
    }
    Ok(Analysis { nodes, warnings })
}

fn contains_file_reference(v: &Value, parent: &str) -> bool {
    match v {
        Value::Object(m) => m.iter().any(|(k, v)| {
            [
                "private-key",
                "certificate",
                "ca-file",
                "key-file",
                "certificate-path",
                "script",
            ]
            .contains(&k.as_str())
                || (k == "path" && !matches!(parent, "ws-opts" | "http-opts"))
                || contains_file_reference(v, k)
        }),
        Value::Array(xs) => xs.iter().any(|v| contains_file_reference(v, parent)),
        _ => false,
    }
}

pub fn source_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return vec![];
    };
    [
        ".mihomo-linux-server",
        ".config/mihomo",
        ".config/clash",
        ".clash",
        ".easymanage/config/proxy-profiles",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect()
}
pub fn sources() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for dir in source_dirs() {
        if fs::symlink_metadata(&dir).map_or(true, |m| !m.is_dir() || m.file_type().is_symlink()) {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                    continue;
                }
                if fs::symlink_metadata(&p).is_ok_and(|m| {
                    m.is_file()
                        && m.len() <= MAX_YAML as u64
                        && m.uid()
                            == fs::metadata("/proc/self")
                                .map(|s| s.uid())
                                .unwrap_or(u32::MAX)
                }) {
                    let id = hex::encode(Sha256::digest(p.as_os_str().as_encoded_bytes()));
                    out.push((id, p));
                }
                if out.len() >= 32 {
                    break;
                }
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.truncate(32);
    out
}
pub fn read_source(id: &str) -> Result<(String, String), String> {
    let path = sources()
        .into_iter()
        .find(|(key, _)| key == id)
        .map(|(_, p)| p)
        .ok_or("本地配置不存在或不可读取")?;
    let before = fs::symlink_metadata(&path).map_err(|_| "本地配置不可读取")?;
    let mut f = fs::File::open(&path).map_err(|_| "本地配置不可读取")?;
    let after = f.metadata().map_err(|_| "本地配置不可读取")?;
    if before.ino() != after.ino() || before.dev() != after.dev() || !after.is_file() {
        return Err("本地配置读取期间发生变化".into());
    }
    let mut text = String::new();
    Read::by_ref(&mut f)
        .take(MAX_YAML as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| "本地配置不是有效 UTF-8")?;
    if text.len() > MAX_YAML {
        return Err("本地配置超过 1 MiB".into());
    }
    Ok((
        text,
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    const NODE: &str = "proxies:\n  - {name: '🇺🇸 US 1', type: ss, server: node.example.com, port: '443', cipher: aes-128-gcm, password: fixture-secret}\n";
    #[tokio::test]
    async fn real_local_catalog_can_be_analyzed_without_returning_node_credentials() {
        if let Some(paths) = std::env::var_os("EASYMANAGE_PROXY_YAML_TESTS") {
            for path in std::env::split_paths(&paths) {
                let text = fs::read_to_string(path).unwrap();
                let analysis = parse(&text).unwrap_or_else(|reason| panic!("{reason}"));
                assert!(!analysis.nodes.is_empty());
                if std::env::var_os("EASYMANAGE_PROXY_CORE_TEST").is_some() {
                    let directory = std::env::temp_dir()
                        .join(format!("proxy-catalog-{}", uuid::Uuid::new_v4()));
                    let vault = super::super::vault::Vault {
                        directory: directory.clone(),
                    };
                    vault.initialize(true).unwrap();
                    let nodes: Vec<_> = analysis
                        .nodes
                        .into_iter()
                        .filter(|n| n.unsupported.is_none())
                        .map(|n| {
                            let mut definition = n.definition;
                            definition["server"] = "8.8.8.8".into();
                            (uuid::Uuid::new_v4(), definition)
                        })
                        .collect();
                    let mut engine = super::super::engine::Engine::start(&vault, &nodes, false)
                        .await
                        .unwrap();
                    engine.shutdown().await;
                    drop(engine);
                    fs::remove_dir_all(directory).unwrap();
                }
            }
        }
    }
    #[test]
    fn imports_only_independent_nodes_and_preserves_identity_when_password_changes() {
        let a=parse(&format!("{NODE}tun: {{enable: true}}\nexternal-controller: 0.0.0.0:9090\nrules: [MATCH,DIRECT]\n")).unwrap();
        assert_eq!(a.nodes[0].region, "US");
        assert_eq!(a.nodes[0].definition["port"], 443);
        assert!(a.nodes[0].definition.get("tun").is_none());
        assert_eq!(
            a.nodes[0].identity,
            parse(&NODE.replace("fixture-secret", "replacement"))
                .unwrap()
                .nodes[0]
                .identity
        );
        for (name, want) in [
            ("日本东京", "JP"),
            ("JP 02", "JP"),
            ("SG 1", "SG"),
            ("台湾 1", "TW"),
            ("🇺🇸 Any", "US"),
            ("AUS Sydney", "OTHER"),
        ] {
            assert_eq!(region(name), want);
        }
    }
    #[test]
    fn rejects_expansion_files_duplicates_and_malformed_secrets_without_echo() {
        for text in [
            "x: &a [*a]\nproxies: []",
            "proxies: [ secret-must-not-echo\n",
        ] {
            let error = parse(text).err().unwrap();
            assert!(!error.contains("secret-must-not-echo"));
        }
        let duplicate = NODE.replace("proxies:\n", "");
        assert!(parse(&format!("{NODE}{duplicate}")).is_err());
        let file = NODE.replace(
            "password: fixture-secret",
            "plugin-opts: {path: /etc/shadow}",
        );
        assert!(parse(&file).is_err());
        let chain = NODE.replace("password: fixture-secret", "dialer-proxy: other");
        assert!(parse(&chain).unwrap().nodes[0].unsupported.is_some());
        assert!(parse(&"x".repeat(MAX_YAML + 1)).is_err());
        assert!(
            parse(&format!(
                "{NODE}extra: {}0{}",
                "[".repeat(40),
                "]".repeat(40)
            ))
            .is_err()
        );
    }
}
