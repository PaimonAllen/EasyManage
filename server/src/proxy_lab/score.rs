//! Manual-only adapters. Unknown, blocked and stale evidence never becomes a zero risk score.
use chrono::{DateTime, Utc};
use easymanage_protocol::NetworkGroup;
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::net::IpAddr;

#[derive(Clone, Serialize)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub api: bool,
    pub direction: &'static str,
}
pub const PROVIDERS: [Provider; 8] = [
    Provider {
        id: "netcoffee",
        name: "Net.Coffee",
        url: "https://ip.net.coffee/ip/",
        api: false,
        direction: "unknown",
    },
    Provider {
        id: "ipcheck",
        name: "IPCheck.ing",
        url: "https://ipcheck.ing/",
        api: false,
        direction: "unknown",
    },
    Provider {
        id: "cleanip",
        name: "CleanIP",
        url: "https://cleanip.io/",
        api: false,
        direction: "purity",
    },
    Provider {
        id: "ipok",
        name: "IPOK",
        url: "https://ipok.io/",
        api: true,
        direction: "risk",
    },
    Provider {
        id: "ipkk",
        name: "IPKK",
        url: "https://ipkk.com/",
        api: false,
        direction: "risk",
    },
    Provider {
        id: "ippure",
        name: "IPPure",
        url: "https://ippure.com/",
        api: true,
        direction: "risk",
    },
    Provider {
        id: "ping0",
        name: "Ping0",
        url: "https://ping0.cc/",
        api: false,
        direction: "unknown",
    },
    Provider {
        id: "ipure",
        name: "IPure.dev",
        url: "https://ipure.dev/",
        api: true,
        direction: "purity",
    },
];

#[derive(Clone, Serialize)]
pub struct Score {
    pub provider: String,
    pub ip: Option<String>,
    pub region: Option<String>,
    pub raw_score: Option<f64>,
    pub purity: Option<f64>,
    pub direction: String,
    pub state: String,
    pub detail: String,
    pub method: String,
    pub checked_at: DateTime<Utc>,
    pub observed_at: Option<DateTime<Utc>>,
}
impl Score {
    pub fn unavailable(id: &str, state: &str, detail: &str) -> Self {
        Self {
            provider: id.into(),
            ip: None,
            region: None,
            raw_score: None,
            purity: None,
            direction: PROVIDERS
                .iter()
                .find(|p| p.id == id)
                .map_or("unknown", |p| p.direction)
                .into(),
            state: state.into(),
            detail: detail.into(),
            method: "api".into(),
            checked_at: Utc::now(),
            observed_at: None,
        }
    }
}
fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
}
pub fn parse(id: &str, body: &Value, ip: IpAddr) -> Score {
    let value = if id == "ipure" {
        body.get("data").unwrap_or(body)
    } else {
        body
    };
    let observed_ip = match id {
        "ipok" => value.pointer("/geo/ip"),
        _ => value.get("ip"),
    }
    .and_then(Value::as_str)
    .and_then(|s| s.parse::<IpAddr>().ok());
    if observed_ip != Some(ip) {
        return Score::unavailable(id, "ip_mismatch", "来源没有确认目标出口 IP，未混入评分");
    }
    let score = match id {
        "ippure" => number(&value["fraudScore"]),
        "ipure" => number(&value["risk"]["purity"]),
        "ipok" => {
            if value["riskAssessment"]["scoreAvailable"] == true {
                number(&value["risk"])
            } else {
                None
            }
        }
        _ => None,
    };
    let mut out = Score::unavailable(
        id,
        if score.is_some() {
            "ok"
        } else {
            "insufficient"
        },
        if score.is_some() {
            ""
        } else {
            "来源未提供当前有效评分；未知不按零风险处理"
        },
    );
    out.ip = Some(ip.to_string());
    out.raw_score = score;
    out.purity = score.map(|n| {
        if out.direction == "risk" {
            100.0 - n
        } else {
            n
        }
    });
    out.region = [
        value.get("countryCode"),
        value.pointer("/geo/countryCode"),
        value.pointer("/geo/country_code"),
        value.pointer("/geo/country"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic()))
    .map(str::to_uppercase);
    out.observed_at = [
        body.get("checked_at"),
        value.get("queriedAt"),
        value.pointer("/riskAssessment/assessedAt"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find_map(|s| DateTime::parse_from_rfc3339(s).ok())
    .map(|d| d.with_timezone(&Utc));
    if value.get("stale") == Some(&Value::Bool(true))
        || body.get("cache").and_then(Value::as_str) == Some("stale")
    {
        out.state = "stale".into();
        out.detail = "来源标记为过期快照；原分保留，不进入当前参考均值".into();
    }
    out
}
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Value, (&'static str, &'static str)> {
    let counters = easymanage_runtime::network::counter(NetworkGroup::HttpService);
    let mut attempt = counters.begin();
    let mut response = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| ("unavailable", "来源连接失败或超时"))?;
    if response.status().as_u16() == 429 {
        return Err(("rate_limited", "来源限流；未自动重试或绕过限制"));
    }
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err(("requires_browser", "来源需要网页验证或授权，请打开原站核验"));
    }
    if !response.status().is_success() {
        return Err(("unavailable", "来源未返回成功响应"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ("unavailable", "来源响应中断"))?
    {
        counters.receive(chunk.len());
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err(("unavailable", "来源响应超出读取范围"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| ("requires_browser", "来源未返回 JSON，可能需要网页验证"))?;
    attempt.finish(true);
    Ok(value)
}
pub async fn egress(client: &reqwest::Client) -> Result<IpAddr, String> {
    for url in [
        "https://ipure.dev/api/myip",
        "https://my.ippure.com/v1/info",
    ] {
        if let Ok(v) = fetch(client, url).await {
            if let Some(ip) = v["ip"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .filter(|ip| super::engine::public_ip(*ip))
            {
                return Ok(ip);
            }
        }
    }
    Err("无法取得经此节点的公网出口 IP；本轮没有给其他 IP 打分".into())
}
pub async fn query(client: &reqwest::Client, provider: &Provider, ip: IpAddr) -> Score {
    if !provider.api {
        let mut score = Score::unavailable(
            provider.id,
            "requires_browser",
            "未确认可供集成的公开评分 API；可打开原站并人工录入结果",
        );
        score.ip = Some(ip.to_string());
        return score;
    }
    let url = match provider.id {
        "ippure" => "https://my.ippure.com/v1/info".into(),
        "ipure" => format!("https://ipure.dev/api/v1/ip/{ip}"),
        "ipok" => format!("https://ipok.io/api/ip?ip={ip}"),
        _ => unreachable!(),
    };
    match fetch(client, &url).await {
        Ok(v) => parse(provider.id, &v, ip),
        Err((state, detail)) => {
            let mut s = Score::unavailable(provider.id, state, detail);
            s.ip = Some(ip.to_string());
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scores_keep_source_polarity_and_require_matching_current_evidence() {
        let ip = "8.8.8.8".parse().unwrap();
        let s = parse(
            "ippure",
            &json!({"ip":"8.8.8.8","fraudScore":20,"countryCode":"US"}),
            ip,
        );
        assert_eq!(s.purity, Some(80.0));
        let s = parse(
            "ipok",
            &json!({"geo":{"ip":"8.8.8.8"},"risk":0,"riskAssessment":{"scoreAvailable":false}}),
            ip,
        );
        assert!(s.purity.is_none());
        assert_eq!(s.state, "insufficient");
        let s = parse(
            "ipure",
            &json!({"data":{"ip":"8.8.8.8","risk":{"purity":85}}}),
            ip,
        );
        assert_eq!(s.purity, Some(85.0));
        assert_eq!(
            parse("ippure", &json!({"ip":"1.1.1.1","fraudScore":0}), ip).state,
            "ip_mismatch"
        );
        assert!(
            parse("ippure", &json!({"ip":"8.8.8.8","fraudScore":101}), ip)
                .raw_score
                .is_none()
        );
    }
}
