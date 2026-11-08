//! Platform-only proxy bootstrap DNS. Never changes host DNS or trusts stale IPs.
use super::engine::public_ip;
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const CAPACITY: usize = 2048;
const MAX_BODY: usize = 65536;
const MAX_TTL: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    NoAddress,
    NxDomain,
    ServerFailure,
    Timeout,
    Transport,
    Tls,
    Invalid,
    Blocked,
    Resource,
    Http(u16),
}
impl Reason {
    fn text(self) -> String {
        match self {
            Self::NoAddress => "没有 A/AAAA 地址记录".into(),
            Self::NxDomain => "NXDOMAIN（域名不存在）".into(),
            Self::ServerFailure => "SERVFAIL（解析服务失败）".into(),
            Self::Timeout => "解析超时".into(),
            Self::Transport => "解析请求失败".into(),
            Self::Tls => "加密 DNS 证书验证失败".into(),
            Self::Invalid => "DNS 响应无效或不匹配".into(),
            Self::Blocked => "仅返回内网或保留地址".into(),
            Self::Resource => "平台解析资源不足".into(),
            Self::Http(code) => format!("加密 DNS 返回 HTTP {code}"),
        }
    }
}
#[derive(Clone, Debug)]
pub(super) struct Failure {
    reasons: Vec<(String, Reason)>,
}
impl Failure {
    pub(super) fn blocked(&self) -> bool {
        self.reasons.iter().all(|(_, r)| *r == Reason::Blocked)
    }
    pub(super) fn detail(&self) -> String {
        format!(
            "节点 DNS 未确认（{}）；未开始节点连通性检测",
            self.reasons
                .iter()
                .map(|(name, r)| format!("{name}：{}", r.text()))
                .collect::<Vec<_>>()
                .join("；")
        )
    }
}
#[derive(Clone, Debug)]
pub(super) struct Resolution {
    pub ip: IpAddr,
    pub note: Option<String>,
}
#[derive(Clone)]
struct Answer {
    ip: IpAddr,
    ttl: Duration,
}
#[derive(Clone)]
struct Entry {
    result: Result<Resolution, Failure>,
    expires: Instant,
}
type Slot = Arc<tokio::sync::Mutex<Option<Entry>>>;
#[derive(Clone)]
struct Provider {
    name: &'static str,
    endpoint: &'static str,
}
const PROVIDERS: [Provider; 2] = [
    Provider {
        name: "AliDNS DoH",
        endpoint: "https://dns.alidns.com/resolve",
    },
    Provider {
        name: "Cloudflare DoH",
        endpoint: "https://cloudflare-dns.com/dns-query",
    },
];
#[derive(Default, Serialize)]
pub(super) struct Stats {
    cache_entries: usize,
    resolutions: u64,
    cache_hits: u64,
    encrypted_fallbacks: u64,
    failures: u64,
}
struct Resolver {
    client: Option<reqwest::Client>,
    bootstrap: Option<reqwest::Client>,
    providers: Vec<Provider>,
    cache: Mutex<HashMap<String, Slot>>,
    resolutions: AtomicU64,
    cache_hits: AtomicU64,
    encrypted_fallbacks: AtomicU64,
    failures: AtomicU64,
}
static RESOLVER: OnceLock<Resolver> = OnceLock::new();
fn resolver() -> &'static Resolver {
    RESOLVER.get_or_init(Resolver::production)
}
pub(super) fn stats() -> Stats {
    RESOLVER
        .get()
        .map(|r| Stats {
            cache_entries: r.cache.lock().unwrap_or_else(|e| e.into_inner()).len(),
            resolutions: r.resolutions.load(Ordering::Relaxed),
            cache_hits: r.cache_hits.load(Ordering::Relaxed),
            encrypted_fallbacks: r.encrypted_fallbacks.load(Ordering::Relaxed),
            failures: r.failures.load(Ordering::Relaxed),
        })
        .unwrap_or_default()
}
pub(super) async fn resolve(host: &str) -> Result<Resolution, Failure> {
    resolver().resolve(host).await
}
fn host_name(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        }))
    .then_some(host)
}
impl Resolver {
    fn production() -> Self {
        let builder = || {
            reqwest::Client::builder()
                .no_proxy()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(4))
                .pool_max_idle_per_host(4)
                .user_agent("EasyManage/0.5 (platform proxy bootstrap DNS)")
        };
        let client = builder().build().ok();
        let bootstrap = builder()
            // TLS still checks the service hostname. Bootstrap addresses avoid
            // asking the broken system resolver to resolve the fallback itself.
            .resolve_to_addrs(
                "dns.alidns.com",
                &[
                    "223.5.5.5:443".parse::<SocketAddr>().unwrap(),
                    "223.6.6.6:443".parse().unwrap(),
                ],
            )
            .resolve_to_addrs(
                "cloudflare-dns.com",
                &[
                    "1.1.1.1:443".parse::<SocketAddr>().unwrap(),
                    "1.0.0.1:443".parse().unwrap(),
                ],
            )
            .user_agent("EasyManage/0.5 (platform proxy bootstrap DNS)")
            .build()
            .ok();
        let mut resolver = Self::new(client, PROVIDERS.to_vec());
        resolver.bootstrap = bootstrap;
        resolver
    }
    fn new(client: Option<reqwest::Client>, providers: Vec<Provider>) -> Self {
        Self {
            client,
            bootstrap: None,
            providers,
            cache: Mutex::new(HashMap::new()),
            resolutions: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            encrypted_fallbacks: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        }
    }
    async fn resolve(&self, host: &str) -> Result<Resolution, Failure> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return if public_ip(ip) {
                Ok(Resolution { ip, note: None })
            } else {
                Err(Failure {
                    reasons: vec![("节点地址".into(), Reason::Blocked)],
                })
            };
        }
        let host = host_name(host).ok_or_else(|| Failure {
            reasons: vec![("节点域名".into(), Reason::Invalid)],
        })?;
        let slot = {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache.len() >= CAPACITY {
                cache.retain(|_, slot| {
                    Arc::strong_count(slot) > 1
                        || slot.try_lock().map_or(true, |value| {
                            value.as_ref().is_some_and(|e| e.expires > Instant::now())
                        })
                });
            }
            if !cache.contains_key(&host) && cache.len() >= CAPACITY {
                return Err(Failure {
                    reasons: vec![("平台".into(), Reason::Resource)],
                });
            }
            cache
                .entry(host.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone()
        };
        // Same-host calls, including overlapping rounds and aliases, share one
        // resolution. A cancelled caller releases the lock without caching.
        let mut entry = slot.lock().await;
        if let Some(cached) = entry.as_ref().filter(|e| e.expires > Instant::now()) {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return cached.result.clone();
        }
        self.resolutions.fetch_add(1, Ordering::Relaxed);
        let (result, ttl) = self.uncached(&host).await;
        if result.is_err() {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        *entry = Some(Entry {
            result: result.clone(),
            expires: Instant::now() + ttl,
        });
        result
    }
    async fn uncached(&self, host: &str) -> (Result<Resolution, Failure>, Duration) {
        let jitter = host
            .bytes()
            .fold(0u64, |n, c| n.wrapping_mul(31).wrapping_add(c as u64))
            % 300;
        tokio::time::sleep(Duration::from_millis(jitter)).await;
        let system = match tokio::time::timeout(
            Duration::from_millis(2500),
            tokio::net::lookup_host((host, 443)),
        )
        .await
        {
            Ok(Ok(values)) => select_ip(values.map(|a| a.ip())).ok_or(Reason::Blocked),
            Ok(Err(error)) => Err(
                if error.to_string().contains("Name or service not known")
                    || error.to_string().contains("No address associated")
                {
                    Reason::NoAddress
                } else {
                    Reason::Transport
                },
            ),
            Err(_) => Err(Reason::Timeout),
        };
        if let Ok(ip) = system {
            return (Ok(Resolution { ip, note: None }), Duration::from_secs(60));
        }
        self.fallback(host, system.unwrap_err()).await
    }
    async fn fallback(
        &self,
        host: &str,
        system: Reason,
    ) -> (Result<Resolution, Failure>, Duration) {
        let Some(client) = &self.client else {
            return (
                Err(Failure {
                    reasons: vec![
                        ("系统 DNS".into(), system),
                        ("加密 DNS".into(), Reason::Resource),
                    ],
                }),
                Duration::from_secs(10),
            );
        };
        let mut reasons = vec![("系统 DNS".into(), system)];
        let mut requests = FuturesUnordered::new();
        for provider in &self.providers {
            let provider = provider.clone();
            let client = client.clone();
            let host = host.to_owned();
            let bootstrap = self.bootstrap.clone();
            requests.push(
                async move {
                    let (v4, v6) = tokio::join!(
                        query_with_bootstrap(&client, bootstrap.as_ref(), &provider, &host, 1),
                        query_with_bootstrap(&client, bootstrap.as_ref(), &provider, &host, 28)
                    );
                    (provider.name, merge(v4, v6))
                }
                .boxed(),
            );
        }
        while let Some((name, result)) = requests.next().await {
            match result {
                Ok(answer) => {
                    self.encrypted_fallbacks.fetch_add(1, Ordering::Relaxed);
                    return (
                        Ok(Resolution {
                            ip: answer.ip,
                            note: Some(format!(
                                "节点 DNS 已通过 {name} 加密回退解析（系统 DNS：{}）",
                                system.text()
                            )),
                        }),
                        answer.ttl,
                    );
                }
                Err((a, b)) => {
                    if a == b {
                        reasons.push((name.into(), a));
                    } else {
                        reasons.push((format!("{name} A"), a));
                        reasons.push((format!("{name} AAAA"), b));
                    }
                }
            }
        }
        (Err(Failure { reasons }), Duration::from_secs(15))
    }
}
fn select_ip(ips: impl Iterator<Item = IpAddr>) -> Option<IpAddr> {
    ips.filter(|ip| public_ip(*ip))
        .min_by_key(|ip| !ip.is_ipv4())
}
fn merge(a: Result<Answer, Reason>, b: Result<Answer, Reason>) -> Result<Answer, (Reason, Reason)> {
    match (a, b) {
        (Ok(a), Ok(b)) => Ok(if a.ip.is_ipv4() { a } else { b }),
        (Ok(a), _) | (_, Ok(a)) => Ok(a),
        (Err(a), Err(b)) => Err((a, b)),
    }
}
#[derive(Deserialize)]
struct Response {
    #[serde(rename = "Status")]
    status: u16,
    #[serde(rename = "TC", default)]
    truncated: bool,
    #[serde(rename = "Question")]
    questions: Questions,
    #[serde(rename = "Answer", default)]
    answers: Vec<Record>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Questions {
    Single(Question),
    Multiple(Vec<Question>),
}
#[derive(Deserialize)]
struct Question {
    name: String,
    #[serde(rename = "type")]
    kind: u16,
}
#[derive(Deserialize)]
struct Record {
    name: String,
    #[serde(rename = "type")]
    kind: u16,
    #[serde(rename = "TTL")]
    ttl: u64,
    data: String,
}
fn parse(bytes: &[u8], host: &str, kind: u16, age: u64) -> Result<Answer, Reason> {
    let response: Response = serde_json::from_slice(bytes).map_err(|_| Reason::Invalid)?;
    let question = match response.questions {
        Questions::Single(question) => question,
        Questions::Multiple(mut questions) if questions.len() == 1 => questions.pop().unwrap(),
        _ => return Err(Reason::Invalid),
    };
    if response.truncated
        || question.kind != kind
        || host_name(&question.name).as_deref() != Some(host)
        || response.answers.len() > 256
    {
        return Err(Reason::Invalid);
    }
    match response.status {
        0 => {}
        3 => return Err(Reason::NxDomain),
        2 => return Err(Reason::ServerFailure),
        _ => return Err(Reason::Invalid),
    }
    let mut owners = HashSet::from([host.to_owned()]);
    for _ in 0..16 {
        let mut changed = false;
        for record in response.answers.iter().filter(|r| r.kind == 5) {
            if host_name(&record.name).is_some_and(|n| owners.contains(&n)) {
                let target = host_name(&record.data).ok_or(Reason::Invalid)?;
                changed |= owners.insert(target);
            }
        }
        if !changed {
            break;
        }
    }
    let records: Vec<_> = response
        .answers
        .iter()
        .filter(|r| host_name(&r.name).is_some_and(|n| owners.contains(&n)))
        .collect();
    let addresses = records
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.data.parse::<IpAddr>().map_err(|_| Reason::Invalid))
        .collect::<Result<Vec<_>, _>>()?;
    if addresses.is_empty() {
        return Err(Reason::NoAddress);
    }
    if addresses.iter().any(|ip| (kind == 1) != ip.is_ipv4()) {
        return Err(Reason::Invalid);
    }
    let ip = select_ip(addresses.into_iter()).ok_or(Reason::Blocked)?;
    let ttl = records
        .iter()
        .filter(|r| r.kind == kind || r.kind == 5)
        .map(|r| r.ttl)
        .min()
        .unwrap_or(0)
        .saturating_sub(age)
        .min(MAX_TTL);
    Ok(Answer {
        ip,
        ttl: Duration::from_secs(ttl),
    })
}
fn http_failure(error: &reqwest::Error) -> Reason {
    if error.is_timeout() {
        Reason::Timeout
    } else if error
        .to_string()
        .to_ascii_lowercase()
        .contains("certificate")
    {
        Reason::Tls
    } else {
        Reason::Transport
    }
}
async fn query_with_bootstrap(
    client: &reqwest::Client,
    bootstrap: Option<&reqwest::Client>,
    provider: &Provider,
    host: &str,
    kind: u16,
) -> Result<Answer, Reason> {
    let result = query(client, provider, host, kind).await;
    if matches!(
        result,
        Err(Reason::Timeout | Reason::Transport | Reason::Tls)
    ) {
        if let Some(bootstrap) = bootstrap {
            return query(bootstrap, provider, host, kind).await;
        }
    }
    result
}
async fn query(
    client: &reqwest::Client,
    provider: &Provider,
    host: &str,
    kind: u16,
) -> Result<Answer, Reason> {
    let mut counter =
        easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::HttpProbe).begin();
    let result = async {
        let mut response = client
            .get(provider.endpoint)
            .query(&[("name", host), ("type", &kind.to_string())])
            .header("Accept", "application/dns-json")
            .send()
            .await
            .map_err(|e| http_failure(&e))?;
        if !response.status().is_success() {
            return Err(Reason::Http(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err(Reason::Invalid);
        }
        let age = response
            .headers()
            .get("age")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| http_failure(&e))? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(Reason::Invalid);
            }
            body.extend_from_slice(&chunk);
        }
        parse(&body, host, kind, age)
    }
    .await;
    counter.finish(result.is_ok());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture(kind: u16, records: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(
            &json!({"Status":0,"Question":[{"name":"node.example.","type":kind}],"Answer":records}),
        )
        .unwrap()
    }
    #[test]
    fn answers_match_questions_and_cname_owners_and_respect_ttl() {
        let bytes = fixture(
            1,
            json!([{"name":"node.example.","type":5,"TTL":45,"data":"edge.example."},{"name":"edge.example.","type":1,"TTL":3600,"data":"8.8.8.8"},{"name":"unrelated.example.","type":1,"TTL":1,"data":"1.1.1.1"}]),
        );
        let answer = parse(&bytes, "node.example", 1, 5).unwrap();
        assert_eq!(answer.ip, "8.8.8.8".parse::<IpAddr>().unwrap());
        assert_eq!(answer.ttl, Duration::from_secs(40));
        assert!(matches!(
            parse(&bytes, "different.example", 1, 0),
            Err(Reason::Invalid)
        ));
        assert!(matches!(
            parse(&bytes, "node.example", 28, 0),
            Err(Reason::Invalid)
        ));
    }
    #[test]
    fn alidns_object_questions_and_cloudflare_array_questions_are_both_validated() {
        let mut value = json!({"Status":0,"Question":{"name":"node.example.","type":1},"Answer":[{"name":"node.example.","type":1,"TTL":600,"data":"8.8.8.8"}]});
        assert_eq!(
            parse(&serde_json::to_vec(&value).unwrap(), "node.example", 1, 0)
                .unwrap()
                .ttl,
            Duration::from_secs(300)
        );
        value["Question"] = json!([value["Question"].clone()]);
        assert!(parse(&serde_json::to_vec(&value).unwrap(), "node.example", 1, 0).is_ok());
        value["Question"] = json!([]);
        assert!(matches!(
            parse(&serde_json::to_vec(&value).unwrap(), "node.example", 1, 0),
            Err(Reason::Invalid)
        ));
    }
    #[test]
    fn negative_and_reserved_answers_never_become_success() {
        for ip in ["127.0.0.1", "10.0.0.1", "198.18.0.1", "169.254.169.254"] {
            let bytes = fixture(
                1,
                json!([{"name":"node.example.","type":1,"TTL":30,"data":ip}]),
            );
            assert!(matches!(
                parse(&bytes, "node.example", 1, 0),
                Err(Reason::Blocked)
            ));
        }
        for (code, reason) in [
            (3, Reason::NxDomain),
            (2, Reason::ServerFailure),
            (0, Reason::NoAddress),
        ] {
            let bytes = serde_json::to_vec(
                &json!({"Status":code,"Question":[{"name":"node.example","type":1}]}),
            )
            .unwrap();
            assert!(matches!(parse(&bytes,"node.example",1,0),Err(r)if r==reason));
        }
        let unrelated = fixture(
            1,
            json!([{"name":"other.example.","type":1,"TTL":30,"data":"8.8.8.8"}]),
        );
        assert!(matches!(
            parse(&unrelated, "node.example", 1, 0),
            Err(Reason::NoAddress)
        ));
    }
    #[tokio::test]
    async fn cache_expires_and_single_flight_avoids_duplicate_fallback_requests() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let received = calls.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let received = received.clone();
                tokio::spawn(async move {
                    let mut bytes = [0; 4096];
                    let n = socket.read(&mut bytes).await.unwrap();
                    let request = String::from_utf8_lossy(&bytes[..n]);
                    received.fetch_add(1, Ordering::Relaxed);
                    let kind = if request.contains("type=28") { 28 } else { 1 };
                    let records = if kind == 1 {
                        json!([{"name":"node.example.","type":1,"TTL":120,"data":"8.8.8.8"}])
                    } else {
                        json!([])
                    };
                    let body = fixture(kind, records);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/dns-json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    socket.write_all(header.as_bytes()).await.unwrap();
                    socket.write_all(&body).await.unwrap();
                });
            }
        });
        let endpoint = Box::leak(format!("http://{address}/resolve").into_boxed_str());
        let resolver = Resolver::new(
            Some(reqwest::Client::builder().no_proxy().build().unwrap()),
            vec![Provider {
                name: "fixture DoH",
                endpoint,
            }],
        );
        // Seed an expired negative result: callers must perform a fresh lookup.
        resolver.cache.lock().unwrap().insert(
            "node.example".into(),
            Arc::new(tokio::sync::Mutex::new(Some(Entry {
                result: Err(Failure {
                    reasons: vec![("old".into(), Reason::Timeout)],
                }),
                expires: Instant::now() - Duration::from_secs(1),
            }))),
        );
        let results =
            futures_util::future::join_all((0..12).map(|_| resolver.resolve("node.example"))).await;
        assert!(results.iter().all(|r| {
            r.as_ref().is_ok_and(|r| {
                r.ip == "8.8.8.8".parse::<IpAddr>().unwrap()
                    && r.note.as_ref().is_some_and(|s| s.contains("fixture DoH"))
            })
        }));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(resolver.resolutions.load(Ordering::Relaxed), 1);
        assert_eq!(resolver.cache_hits.load(Ordering::Relaxed), 11);
        server.abort();
    }
    #[tokio::test]
    async fn literal_private_addresses_never_query_fallbacks() {
        let resolver = Resolver::new(None, vec![]);
        assert!(resolver.resolve("127.0.0.1").await.unwrap_err().blocked());
        assert_eq!(resolver.resolutions.load(Ordering::Relaxed), 0);
        assert!(resolver.resolve("8.8.8.8").await.is_ok());
        assert!(resolver.resolve("bad/name").await.is_err());
    }
}
