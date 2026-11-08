//! Bounded same-service fallbacks. No cookies, body capture, CAPTCHA or exit changes.
use super::{engine::TestSite, history::SiteFeedback};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::time::Duration;

fn retry_at(value: Option<&str>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Ok(seconds) = value.trim().parse::<i64>() {
        return (seconds >= 0).then(|| {
            ChronoDuration::try_seconds(seconds)
                .and_then(|delta| now.checked_add_signed(delta))
                .unwrap_or(DateTime::<Utc>::MAX_UTC)
        });
    }
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|at| at.with_timezone(&Utc).max(now))
}
struct Reply {
    status: u16,
    ms: i32,
    retry: Option<DateTime<Utc>>,
}
#[derive(Clone, Copy, Debug)]
enum Failure {
    Network,
    Tls,
    Local,
}
fn classify_error(error: &reqwest::Error) -> Failure {
    use std::error::Error;
    let mut source: Option<&(dyn Error + 'static)> = Some(error);
    let mut tls = false;
    while let Some(e) = source {
        if e.downcast_ref::<std::io::Error>()
            .is_some_and(|io| matches!(io.raw_os_error(), Some(12 | 23 | 24 | 105)))
        {
            return Failure::Local;
        }
        // Only classify in memory; raw errors can contain sensitive proxy URLs.
        tls |= e.to_string().to_ascii_lowercase().contains("certificate");
        source = e.source();
    }
    if error.is_builder() {
        Failure::Local
    } else if tls {
        Failure::Tls
    } else {
        Failure::Network
    }
}
async fn head(client: &reqwest::Client, site: TestSite, index: usize) -> Result<Reply, Failure> {
    for attempt_index in 0..2 {
        let counters =
            easymanage_runtime::network::counter(easymanage_protocol::NetworkGroup::HttpProbe);
        let mut attempt = counters.begin();
        let start = tokio::time::Instant::now();
        match client.head(site.url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let retry = retry_at(
                    response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok()),
                    Utc::now(),
                );
                attempt.finish(response.status().is_success());
                // HEAD only: never read, retain or log a website response body.
                return Ok(Reply {
                    status,
                    ms: start.elapsed().as_millis().clamp(1, 8000) as i32,
                    retry,
                });
            }
            Err(error) => {
                let kind = classify_error(&error);
                attempt.finish(false);
                // A single transport retry after a stagger. Never retry a received
                // 429/challenge, certificate failure or platform resource exhaustion.
                if attempt_index == 0 && matches!(kind, Failure::Network) {
                    tokio::time::sleep(Duration::from_millis(400 + index as u64 * 37)).await;
                } else {
                    return Err(kind);
                }
            }
        }
    }
    unreachable!()
}
fn failed(kind: Failure, mut info: SiteFeedback) -> (i32, SiteFeedback) {
    info.outcome = match kind {
        Failure::Local => "local_error",
        Failure::Tls => "tls_error",
        Failure::Network => "transport_failed",
    }
    .into();
    (
        if matches!(kind, Failure::Local) {
            -2
        } else {
            -1
        },
        info,
    )
}
fn expected(reply: &Reply, site: TestSite) -> bool {
    site.expected.parse::<u16>().ok() == Some(reply.status)
}
fn limited(reply: &Reply) -> bool {
    reply.status == 429 || (matches!(reply.status, 403 | 503) && reply.retry.is_some())
}
pub async fn recover(
    client: &reqwest::Client,
    index: usize,
    primary: TestSite,
    backup: TestSite,
    previous: Option<SiteFeedback>,
) -> Option<(i32, SiteFeedback)> {
    let now = Utc::now();
    let mut info = previous.unwrap_or(SiteFeedback {
        site: index,
        primary_status: None,
        fallback_status: None,
        retry_at: None,
        fallback_retry_at: None,
        rate_limited: false,
        outcome: String::new(),
    });
    let cooling = info.retry_at.is_some_and(|at| at > now);
    if !cooling {
        let reply = match head(client, primary, index).await {
            Ok(reply) => reply,
            Err(kind) => {
                info.primary_status = None;
                info.rate_limited = false;
                info.fallback_status = None;
                info.retry_at = None;
                return Some(failed(kind, info));
            }
        };
        info.primary_status = Some(reply.status);
        info.fallback_status = None;
        info.retry_at = None;
        if expected(&reply, primary) {
            info.rate_limited = false;
            info.outcome = "primary_ok".into();
            return Some((reply.ms, info));
        }
        info.rate_limited = limited(&reply);
        if info.rate_limited {
            info.retry_at = reply.retry.or(Some(now + ChronoDuration::seconds(60)));
        }
        if !matches!(reply.status, 301..=308 | 401 | 403 | 404 | 405 | 429 | 500..=599) {
            info.outcome = "http_error".into();
            return Some((-2, info));
        }
    }
    let origin = |url: &str| reqwest::Url::parse(url).ok().map(|u| u.origin());
    // A response's Retry-After applies to its origin. Never request that origin
    // during its cooldown, even if a different path was configured as backup.
    if info.fallback_retry_at.is_some_and(|at| at > now)
        || (info.retry_at.is_some_and(|at| at > now) && origin(primary.url) == origin(backup.url))
    {
        info.outcome = "rate_limited".into();
        return Some((-2, info));
    }
    tokio::time::sleep(Duration::from_millis(150 + index as u64 * 17)).await;
    match head(client, backup, index).await {
        Ok(reply) => {
            info.fallback_status = Some(reply.status);
            info.fallback_retry_at = if limited(&reply) {
                reply
                    .retry
                    .or(Some(Utc::now() + ChronoDuration::seconds(60)))
            } else {
                None
            };
            if expected(&reply, backup) {
                info.outcome = "fallback_ok".into();
                Some((reply.ms, info))
            } else {
                info.rate_limited |= limited(&reply);
                info.outcome = if info.rate_limited {
                    "rate_limited"
                } else if matches!(reply.status, 500..=599) {
                    "service_unavailable"
                } else {
                    "http_restricted"
                }
                .into();
                // An authenticated HTTPS reply proves transport, but an error,
                // redirect/challenge or site outage does not prove node failure.
                Some((-2, info))
            }
        }
        Err(kind) => {
            // Keep the primary's real HTTP evidence; backup transport failure
            // cannot turn a known site restriction into a confirmed node outage.
            info.outcome = if info.rate_limited {
                "rate_limited"
            } else if cooling || info.primary_status.is_some() {
                "http_restricted"
            } else {
                return Some(failed(kind, info));
            }
            .into();
            Some((-2, info))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn site(status: u16, retry: Option<&str>) -> (TestSite, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Box::leak(format!("http://{}/", listener.local_addr().unwrap()).into_boxed_str());
        let retry = retry.map(str::to_owned);
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            assert!(bytes[..n].starts_with(b"HEAD /"));
            let header = retry
                .map(|r| format!("Retry-After: {r}\r\n"))
                .unwrap_or_default();
            socket.write_all(format!("HTTP/1.1 {status} Fixture\r\n{header}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        });
        (
            TestSite {
                name: "fixture",
                url,
                expected: "200",
            },
            task,
        )
    }
    #[tokio::test]
    async fn a_transient_transport_drop_is_retried_but_not_painted_red() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let primary = TestSite {
            name: "fixture",
            url: Box::leak(format!("http://{}/", listener.local_addr().unwrap()).into_boxed_str()),
            expected: "200",
        };
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            drop(socket);
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            socket.read(&mut bytes).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let (ms, info) = recover(&client, 0, primary, primary, None).await.unwrap();
        assert!(ms >= 0);
        assert_eq!(info.outcome, "primary_ok");
        task.await.unwrap();
    }
    #[tokio::test]
    async fn website_service_errors_do_not_prove_a_node_outage() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let (primary, p) = site(503, None).await;
        let (backup, b) = site(503, None).await;
        let (value, info) = recover(&client, 0, primary, backup, None).await.unwrap();
        assert_eq!(value, -2);
        assert_eq!(info.outcome, "service_unavailable");
        p.await.unwrap();
        b.await.unwrap();
    }
    #[test]
    fn retry_after_accepts_seconds_and_http_dates_without_preserving_headers() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        assert_eq!(
            (retry_at(Some("600"), now).unwrap() - now).num_seconds(),
            600
        );
        assert!(retry_at(Some("Wed, 21 Oct 2037 07:28:00 GMT"), now).is_some());
        assert!(retry_at(Some("9223372036854775807"), now).is_some());
        assert!(retry_at(Some("-10"), now).is_none());
        assert!(retry_at(Some("private-invalid-header"), now).is_none());
    }
    #[tokio::test]
    async fn rate_limited_origin_falls_back_once_and_survives_as_cooldown_evidence() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let (primary, p) = site(429, Some("600")).await;
        let (backup, b) = site(200, None).await;
        let (ms, info) = recover(&client, 0, primary, backup, None).await.unwrap();
        assert!(ms > 0);
        assert_eq!(info.primary_status, Some(429));
        assert_eq!(info.fallback_status, Some(200));
        assert!(info.rate_limited);
        assert_eq!(info.outcome, "fallback_ok");
        assert!(info.retry_at.unwrap() > Utc::now());
        p.await.unwrap();
        b.await.unwrap();
        let (backup, b) = site(200, None).await;
        let (ms, next) = recover(&client, 0, primary, backup, Some(info))
            .await
            .unwrap();
        assert!(ms > 0);
        assert_eq!(next.primary_status, Some(429));
        b.await.unwrap();
    }
    #[tokio::test]
    async fn cooldown_does_not_become_node_failure_or_retry_the_same_origin() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let (primary, p) = site(429, Some("600")).await;
        let mut backup = primary;
        backup.url = Box::leak(format!("{}backup", primary.url).into_boxed_str());
        let (ms, info) = recover(&client, 0, primary, backup, None).await.unwrap();
        assert_eq!(ms, -2);
        assert_eq!(info.fallback_status, None);
        assert_eq!(info.outcome, "rate_limited");
        p.await.unwrap();
    }
    #[tokio::test]
    async fn both_endpoints_rate_limited_remain_unknown() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let (primary, p) = site(429, Some("600")).await;
        let (backup, b) = site(429, Some("1200")).await;
        let (ms, info) = recover(&client, 0, primary, backup, None).await.unwrap();
        assert_eq!(ms, -2);
        assert!(info.rate_limited);
        assert_eq!(info.fallback_status, Some(429));
        assert!(info.retry_at.is_some() && info.fallback_retry_at.is_some());
        p.await.unwrap();
        b.await.unwrap();
    }
    #[tokio::test]
    async fn forbidden_is_explicitly_restricted_not_fabricated_as_rate_limiting() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let (primary, p) = site(403, None).await;
        let (backup, b) = site(403, None).await;
        let (ms, info) = recover(&client, 0, primary, backup, None).await.unwrap();
        assert_eq!(ms, -2);
        assert!(!info.rate_limited);
        assert_eq!(info.outcome, "http_restricted");
        p.await.unwrap();
        b.await.unwrap();
    }
}
