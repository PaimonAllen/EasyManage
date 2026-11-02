//! Content-free, bounded accounting at application boundaries, never packet capture.
use axum::{
    Json, Router,
    body::{Body, Bytes, HttpBody},
    extract::{Request, State},
    middleware::Next,
    response::Response,
    routing::get,
};
use easymanage_protocol::NetworkGroup;
use easymanage_runtime::network::{self, Attempt, Counters};
use http_body::{Frame, SizeHint};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::{
    io::{self, Read, Write},
    ops::{Deref, DerefMut},
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};

pub async fn account(request: Request, next: Next) -> Response {
    let group = if matches!(
        request.uri().path(),
        "/api/v1/agent/live"
            | "/api/v1/agent/report"
            | "/api/v1/agent/enroll"
            | "/api/v1/agent/control"
    ) {
        NetworkGroup::HttpAgent
    } else {
        NetworkGroup::HttpBrowser
    };
    let counters = network::counter(group);
    let attempt = counters.begin();
    let head = request.method() == axum::http::Method::HEAD;
    let (parts, body) = request.into_parts();
    let request = Request::from_parts(
        parts,
        Body::new(MeteredBody {
            body,
            counters,
            incoming: true,
            attempt: None,
            ok: true,
        }),
    );
    let response = next.run(request).await;
    let ok = response.status().as_u16() < 400;
    let (parts, body) = response.into_parts();
    let mut measured = MeteredBody {
        body,
        counters,
        incoming: false,
        attempt: Some(attempt),
        ok,
    };
    if head || measured.body.is_end_stream() {
        measured.finish();
    }
    Response::from_parts(parts, Body::new(measured))
}

struct MeteredBody {
    body: Body,
    counters: &'static Counters,
    incoming: bool,
    attempt: Option<Attempt<'static>>,
    ok: bool,
}
impl MeteredBody {
    fn finish(&mut self) {
        if let Some(mut attempt) = self.attempt.take() {
            attempt.finish(self.ok);
        }
    }
}
impl HttpBody for MeteredBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        let frame = Pin::new(&mut this.body).poll_frame(cx);
        match &frame {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    if this.incoming {
                        this.counters.receive(data.len());
                    } else {
                        this.counters.submit(data.len());
                    }
                }
            }
            Poll::Ready(Some(Err(_))) => {
                this.ok = false;
                this.finish();
            }
            Poll::Ready(None) => this.finish(),
            Poll::Pending => {}
        }
        if this.body.is_end_stream() {
            this.finish();
        }
        frame
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// Limit and account external service bodies, keeping auth headers out of metrics.
pub struct ServiceResponse {
    pub status: reqwest::StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}
impl ServiceResponse {
    pub fn json(&self) -> anyhow::Result<Value> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}
pub async fn service_request(
    request: reqwest::RequestBuilder,
    group: NetworkGroup,
) -> anyhow::Result<ServiceResponse> {
    let counters = network::counter(group);
    let mut attempt = counters.begin();
    let mut response = request.send().await?;
    let status = response.status();
    let headers = response.headers().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        counters.receive(chunk.len());
        if body.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
            anyhow::bail!("service response exceeds bounded limit");
        }
        body.extend_from_slice(&chunk);
    }
    attempt.finish(status.is_success());
    Ok(ServiceResponse {
        status,
        headers,
        body,
    })
}

pub struct MeasuredChannel {
    channel: ssh2::Channel,
    counters: &'static Counters,
    attempt: Attempt<'static>,
    failed: bool,
}
impl MeasuredChannel {
    // A nonce-tagged, hash-verified application receipt proves command completion
    // even if a detached helper keeps the SSH output stream open.
    pub(crate) fn confirm_completion(&mut self) { self.attempt.finish(true); }
}
impl Deref for MeasuredChannel {
    type Target = ssh2::Channel;
    fn deref(&self) -> &Self::Target {
        &self.channel
    }
}
impl DerefMut for MeasuredChannel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.channel
    }
}
impl Read for MeasuredChannel {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let result = self.channel.read(buffer);
        match &result {
            Ok(n) => self.counters.receive(*n),
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => self.failed = true,
            _ => {}
        }
        result
    }
}
impl Write for MeasuredChannel {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let result = self.channel.write(buffer);
        match &result {
            Ok(n) => self.counters.submit(*n),
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => self.failed = true,
            _ => {}
        }
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        self.channel.flush()
    }
}
impl Drop for MeasuredChannel {
    fn drop(&mut self) {
        self.attempt.finish(
            !self.failed && self.channel.eof() && matches!(self.channel.exit_status(), Ok(0)),
        );
    }
}
pub fn ssh_channel(session: &ssh2::Session) -> Result<MeasuredChannel, ssh2::Error> {
    let counters = network::counter(NetworkGroup::SshExec);
    let attempt = counters.begin();
    Ok(MeasuredChannel {
        channel: session.channel_session()?,
        counters,
        attempt,
        failed: false,
    })
}
pub fn scp_send(
    session: &ssh2::Session,
    path: &Path,
    mode: i32,
    size: u64,
) -> Result<MeasuredChannel, ssh2::Error> {
    let counters = network::counter(NetworkGroup::SshScp);
    let attempt = counters.begin();
    Ok(MeasuredChannel {
        channel: session.scp_send(path, mode, size, None)?,
        counters,
        attempt,
        failed: false,
    })
}

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/api/v1/platform-network", get(status))
        .with_state(pool)
}
async fn status(State(pool): State<PgPool>) -> Json<Value> {
    let rows=sqlx::query("SELECT n.id,n.monitoring_paused,COALESCE(NULLIF(n.display_name,''),n.hostname) AS hostname,n.last_seen,m.metrics->'network_activity' AS network_activity FROM nodes n LEFT JOIN metrics_latest m ON m.node_id=n.id WHERE n.deleted_at IS NULL ORDER BY n.hostname")
        .fetch_all(&pool).await;
    let available = rows.is_ok();
    let agents: Vec<_> = rows
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            json!({
                "node_id":row.get::<uuid::Uuid,_>("id"),"hostname":row.get::<String,_>("hostname"),
                "last_seen":row.get::<chrono::DateTime<chrono::Utc>,_>("last_seen"),
                "monitoring_paused":row.get::<bool,_>("monitoring_paused"),
                "activity":row.get::<Option<Value>,_>("network_activity")
            })
        })
        .collect();
    Json(
        json!({"generated_at":chrono::Utc::now(),"server":network::snapshot(false),"agents":agents,"agents_available":available}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn streaming_body_counts_consumed_bytes_without_buffering_or_losing_size_hint() {
        let counters = network::counter(NetworkGroup::HttpBrowser);
        let before = network::snapshot(false)
            .groups
            .into_iter()
            .find(|g| g.group == NetworkGroup::HttpBrowser)
            .unwrap();
        let body = MeteredBody {
            body: Body::from("fixture"),
            counters,
            incoming: false,
            attempt: Some(counters.begin()),
            ok: true,
        };
        assert_eq!(body.size_hint().exact(), Some(7));
        assert_eq!(
            axum::body::to_bytes(Body::new(body), 32).await.unwrap(),
            "fixture"
        );
        let after = network::snapshot(false)
            .groups
            .into_iter()
            .find(|g| g.group == NetworkGroup::HttpBrowser)
            .unwrap();
        assert_eq!(after.submitted_bytes - before.submitted_bytes, 7);
        assert_eq!(after.completed - before.completed, 1);
    }
    #[test]
    fn dropping_an_unconsumed_response_accounts_cancellation_without_fake_payload_bytes() {
        let counters = network::counter(NetworkGroup::HttpProbe);
        let snapshot = || {
            network::snapshot(false)
                .groups
                .into_iter()
                .find(|g| g.group == NetworkGroup::HttpProbe)
                .unwrap()
        };
        let before = snapshot();
        drop(MeteredBody {
            body: Body::from("not consumed"),
            counters,
            incoming: false,
            attempt: Some(counters.begin()),
            ok: true,
        });
        let after = snapshot();
        assert_eq!(after.failed - before.failed, 1);
        assert_eq!(after.submitted_bytes, before.submitted_bytes);
    }
}
