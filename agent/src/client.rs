use crate::config::AgentConfig;
use anyhow::{Context, Result, bail};
use easymanage_protocol::{
    AGENT_TOKEN_HEADER, AgentControl, AgentControlRequest, AgentControlResponse, AgentLiveReport,
    AgentReport, AgentReportResponse, CollectionPolicy, ENROLLMENT_TOKEN_HEADER, EnrollmentRequest,
    EnrollmentResponse, NetworkGroup,
};

#[derive(Debug)]
pub enum ReportOutcome {
    Accepted,
    Unauthorized,
}

pub struct ServerClient {
    http: reqwest::Client,
    base_url: String,
    enrollment_token: String,
    policy: std::sync::Mutex<Option<CollectionPolicy>>,
    control: std::sync::Mutex<Option<AgentControl>>,
    software: std::sync::Mutex<Option<(uuid::Uuid, chrono::DateTime<chrono::Utc>)>>,
}

impl ServerClient {
    pub fn new(config: &AgentConfig) -> Result<Self> {
        easymanage_runtime::network::counter(NetworkGroup::AgentLive);
        let ca_pem = std::fs::read(&config.ca_certificate).with_context(|| {
            format!(
                "failed to read CA certificate {}",
                config.ca_certificate.display()
            )
        })?;
        let certificate =
            reqwest::Certificate::from_pem(&ca_pem).context("failed to parse CA certificate")?;
        let http = reqwest::Client::builder()
            .add_root_certificate(certificate)
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            // Paused control polls are 60s apart, exactly the SSH pipe idle
            // limit. Retire idle HTTP streams first to avoid reusing a pipe
            // being closed at that boundary; new streams reuse the same SSH.
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .timeout(config.request_timeout)
            .user_agent(concat!("EasyManage-Agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTPS client")?;

        Ok(Self {
            software: std::sync::Mutex::new(None),
            http,
            base_url: config.server_url.clone(),
            enrollment_token: config.enrollment_token.clone(),
            policy: std::sync::Mutex::new(None),
            control: std::sync::Mutex::new(None),
        })
    }

    pub async fn enroll(&self, request: &EnrollmentRequest) -> Result<EnrollmentResponse> {
        let (status, body) = self
            .post_bytes(
                "enroll",
                ENROLLMENT_TOKEN_HEADER,
                &self.enrollment_token,
                serde_json::to_vec(request)?,
                NetworkGroup::AgentEnroll,
            )
            .await
            .context("failed to reach enrollment endpoint")?;
        if !status.is_success() {
            bail!("server rejected agent enrollment with HTTP {status}");
        }
        serde_json::from_slice(&body).context("invalid enrollment response from server")
    }

    pub async fn report(&self, token: &str, report: &AgentReport) -> Result<ReportOutcome> {
        let key = report
            .metrics
            .inventory_collected_at
            .map(|at| (report.node_id, at));
        let reuse = key.is_some()
            && self
                .software
                .lock()
                .ok()
                .is_some_and(|cached| *cached == key);
        let mut compact;
        let outgoing = if reuse {
            compact = report.clone();
            compact.metrics.software_packages.clear();
            compact.reuse_software_inventory = true;
            compact.protocol_version = easymanage_protocol::SOFTWARE_CACHE_PROTOCOL_VERSION;
            &compact
        } else {
            report
        };
        let (mut status, mut body) = self
            .post_bytes(
                "report",
                AGENT_TOKEN_HEADER,
                token,
                serde_json::to_vec(outgoing)?,
                NetworkGroup::AgentReport,
            )
            .await
            .context("failed to reach metrics endpoint")?;

        // A restored/moved database can lose its cache. Retry the durable full
        // report once, without re-enrolling, discarding history or new SSH.
        if reuse && matches!(status,reqwest::StatusCode::CONFLICT | reqwest::StatusCode::BAD_REQUEST) {
            if let Ok(mut cached) = self.software.lock() {
                *cached = None;
            }
            (status, body) = self
                .post_bytes(
                    "report",
                    AGENT_TOKEN_HEADER,
                    token,
                    serde_json::to_vec(report)?,
                    NetworkGroup::AgentReport,
                )
                .await
                .context("failed to restore software inventory")?;
        }

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(ReportOutcome::Unauthorized);
        }
        if !status.is_success() {
            bail!("server rejected metrics report with HTTP {status}");
        }
        let accepted: AgentReportResponse =
            serde_json::from_slice(&body).context("invalid metrics response from server")?;
        if let Ok(mut cached) = self.software.lock() {
            *cached = if accepted.software_inventory_cached == Some(true) {
                key
            } else {
                None
            };
        }
        if let Some(control) = accepted.agent_control {
            self.observe_control(control);
        }
        if let Some(policy) = accepted.collection_policy {
            if policy.validate().is_ok() {
                if let Ok(mut latest) = self.policy.lock() {
                    *latest = Some(policy);
                }
            } else {
                tracing::warn!("server collection policy rejected; retaining previous intervals");
            }
        }
        Ok(ReportOutcome::Accepted)
    }

    pub fn collection_policy(&self) -> Option<CollectionPolicy> {
        self.policy.lock().ok().and_then(|policy| policy.clone())
    }

    fn observe_control(&self, next: AgentControl) {
        if next.revision > i64::MAX as u64 {
            return;
        }
        if let Ok(mut current) = self.control.lock() {
            if current
                .as_ref()
                .is_none_or(|old| next.revision > old.revision || next == *old)
            {
                *current = Some(next);
            }
        }
    }

    pub fn agent_control(&self) -> Option<AgentControl> {
        self.control.lock().ok().and_then(|value| value.clone())
    }

    pub async fn poll_control(
        &self,
        token: &str,
        node_id: uuid::Uuid,
        applied: &AgentControl,
    ) -> Result<()> {
        let (status, body) = self
            .post_bytes(
                "control",
                AGENT_TOKEN_HEADER,
                token,
                serde_json::to_vec(&AgentControlRequest {
                    node_id,
                    applied: applied.clone(),
                    supports_proxy_collection: true,
                })?,
                NetworkGroup::AgentControl,
            )
            .await
            .context("failed to reach monitoring control endpoint")?;
        if !status.is_success() {
            bail!("monitoring control rejected with HTTP {status}");
        }
        let response: AgentControlResponse =
            serde_json::from_slice(&body).context("invalid monitoring control response")?;
        self.observe_control(response.control);
        if response.collection_policy.validate().is_ok() {
            if let Ok(mut policy) = self.policy.lock() {
                *policy = Some(response.collection_policy);
            }
        }
        Ok(())
    }

    pub async fn report_live(
        &self,
        token: &str,
        report: &AgentLiveReport,
    ) -> Result<ReportOutcome> {
        let (status, _) = self
            .post_bytes(
                "live",
                AGENT_TOKEN_HEADER,
                token,
                serde_json::to_vec(report)?,
                NetworkGroup::AgentLive,
            )
            .await
            .context("failed to reach live metrics endpoint")?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(ReportOutcome::Unauthorized);
        }
        if !status.is_success() {
            bail!("server rejected live metrics with HTTP {status}");
        }
        Ok(ReportOutcome::Accepted)
    }

    async fn post_bytes(
        &self,
        path: &str,
        header: &'static str,
        token: &str,
        body: Vec<u8>,
        group: NetworkGroup,
    ) -> Result<(reqwest::StatusCode, Vec<u8>)> {
        let counters = easymanage_runtime::network::counter(group);
        let mut attempt = counters.begin();
        counters.submit(body.len()); // submitted payload budget, NOT proof of wire delivery
        let mut request = self
            .http
            .post(format!("{}/api/v1/agent/{path}", self.base_url))
            .header(header, token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if group == NetworkGroup::AgentLive {
            request = request.timeout(std::time::Duration::from_secs(2));
        }
        let mut response = request.send().await?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            counters.receive(chunk.len());
            if bytes.len().saturating_add(chunk.len()) > 256 * 1024 {
                bail!("platform response exceeds bounded limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        attempt.finish(status.is_success());
        Ok((status, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitoring_control_never_rolls_back_or_changes_at_the_same_revision() {
        let client = ServerClient {
            software: std::sync::Mutex::new(None),
            http: reqwest::Client::new(),
            base_url: String::new(),
            enrollment_token: String::new(),
            policy: std::sync::Mutex::new(None),
            control: std::sync::Mutex::new(None),
        };
        let paused = AgentControl {
            revision: 3,
            paused: true,
        };
        client.observe_control(paused.clone());
        client.observe_control(AgentControl {
            revision: 2,
            paused: false,
        });
        client.observe_control(AgentControl {
            revision: 3,
            paused: false,
        });
        client.observe_control(AgentControl {
            revision: u64::MAX,
            paused: false,
        });
        assert_eq!(client.agent_control(), Some(paused));
        let resumed = AgentControl {
            revision: 4,
            paused: false,
        };
        client.observe_control(resumed.clone());
        client.observe_control(resumed.clone());
        assert_eq!(client.agent_control(), Some(resumed));
    }
}
