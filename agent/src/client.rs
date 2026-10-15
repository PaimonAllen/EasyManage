use crate::config::AgentConfig;
use anyhow::{Context, Result, bail};
use easymanage_protocol::{
    AGENT_TOKEN_HEADER, AgentLiveReport, AgentReport, AgentReportResponse, CollectionPolicy,
    ENROLLMENT_TOKEN_HEADER, EnrollmentRequest, EnrollmentResponse,
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
}

impl ServerClient {
    pub fn new(config: &AgentConfig) -> Result<Self> {
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
            .timeout(config.request_timeout)
            .user_agent(concat!("EasyManage-Agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTPS client")?;

        Ok(Self {
            http,
            base_url: config.server_url.clone(),
            enrollment_token: config.enrollment_token.clone(),
            policy: std::sync::Mutex::new(None),
        })
    }

    pub async fn enroll(&self, request: &EnrollmentRequest) -> Result<EnrollmentResponse> {
        let response = self
            .http
            .post(format!("{}/api/v1/agent/enroll", self.base_url))
            .header(ENROLLMENT_TOKEN_HEADER, &self.enrollment_token)
            .json(request)
            .send()
            .await
            .context("failed to reach enrollment endpoint")?;
        let status = response.status();
        if !status.is_success() {
            bail!("server rejected agent enrollment with HTTP {status}");
        }
        response
            .json()
            .await
            .context("invalid enrollment response from server")
    }

    pub async fn report(&self, token: &str, report: &AgentReport) -> Result<ReportOutcome> {
        let response = self
            .http
            .post(format!("{}/api/v1/agent/report", self.base_url))
            .header(AGENT_TOKEN_HEADER, token)
            .json(report)
            .send()
            .await
            .context("failed to reach metrics endpoint")?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(ReportOutcome::Unauthorized);
        }
        let status = response.status();
        if !status.is_success() {
            bail!("server rejected metrics report with HTTP {status}");
        }
        let accepted: AgentReportResponse = response
            .json()
            .await
            .context("invalid metrics response from server")?;
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

    pub async fn report_live(
        &self,
        token: &str,
        report: &AgentLiveReport,
    ) -> Result<ReportOutcome> {
        let response = self
            .http
            .post(format!("{}/api/v1/agent/live", self.base_url))
            .header(AGENT_TOKEN_HEADER, token)
            .json(report)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .context("failed to reach live metrics endpoint")?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(ReportOutcome::Unauthorized);
        }
        let status = response.status();
        if !status.is_success() {
            bail!("server rejected live metrics with HTTP {status}");
        }
        Ok(ReportOutcome::Accepted)
    }
}
