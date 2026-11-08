//! 480 real three-minute slots; missing observations never count as healthy.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

pub const PERIOD: i64 = 180;
pub const SLOTS: usize = 480;
fn legacy_policy() -> u8 {
    1
}
pub const POLICY: u8 = 5;
pub const RETENTION_DAYS: i64 = 7;
#[derive(Clone, Serialize, Deserialize)]
pub struct NodeLink {
    pub state: String,
    pub latency_ms: Option<u64>,
    pub detail: String,
    pub kind: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SiteFeedback {
    pub site: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_retry_at: Option<DateTime<Utc>>,
    pub rate_limited: bool,
    pub outcome: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct HttpFallback {
    pub head_status: u16,
    pub get_status: Option<u16>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: Uuid,
    pub state: String,
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_fallback: Option<HttpFallback>,
    pub detail: String,
    #[serde(default = "legacy_policy")]
    pub policy: u8,
    /// Versioned catalog order; >=0 verified milliseconds, -1 failed, -2 unconfirmed.
    /// Never persist URLs, response bodies or repeated per-site strings per round.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<NodeLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub site_feedback: Vec<SiteFeedback>,
}
pub fn window(now: DateTime<Utc>) -> i64 {
    (now.timestamp() / PERIOD - (SLOTS as i64 - 1)) * PERIOD
}
pub fn histories(
    ids: &[Uuid],
    start: i64,
    rounds: &[(i64, Value)],
    link_mode: bool,
) -> HashMap<Uuid, (String, Option<Observation>, Option<DateTime<Utc>>, usize)> {
    histories_with_slots(ids, start, rounds, link_mode, SLOTS)
}
pub fn histories_with_slots(
    ids: &[Uuid],
    start: i64,
    rounds: &[(i64, Value)],
    link_mode: bool,
    count: usize,
) -> HashMap<Uuid, (String, Option<Observation>, Option<DateTime<Utc>>, usize)> {
    let mut nodes: HashMap<_, _> = ids
        .iter()
        .map(|id| (*id, (vec![b'-'; count], None, None, vec![false; count])))
        .collect();
    for (at, values) in rounds {
        let index = (*at - start) / PERIOD;
        if *at < start || index < 0 || index >= count as i64 {
            continue;
        }
        if let Some(items) = values.as_array() {
            for item in items {
                if let Ok(mut o) = serde_json::from_value::<Observation>(item.clone()) {
                    if link_mode {
                        let Some(link) = o.link.clone() else { continue };
                        o.state = link.state;
                        o.latency_ms = link.latency_ms;
                        o.detail = link.detail;
                        o.sites.clear();
                    }
                    if let Some((slots, latest, last_at, legacy)) = nodes.get_mut(&o.id) {
                        legacy[index as usize] = o.policy < POLICY;
                        slots[index as usize] = match o.state.as_str() {
                            "up" => b'u',
                            "slow" => b's',
                            "down" => b'd',
                            "unsupported" => b'x',
                            _ => b'?',
                        };
                        *latest = Some(o);
                        *last_at = DateTime::from_timestamp(*at, 0);
                    }
                }
            }
        }
    }
    nodes
        .into_iter()
        .map(|(id, (slots, latest, at, legacy))| {
            (
                id,
                (
                    String::from_utf8(slots).unwrap(),
                    latest,
                    at,
                    legacy.into_iter().filter(|v| *v).count(),
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_slots_keep_gaps_and_unknown_separate_from_outages() {
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        let start = 1_800_000;
        let observation = |state: &str| serde_json::json!([{"id":id,"state":state,"latency_ms":null,"detail":""}]);
        let rounds = vec![
            (start - 180, observation("down")),
            (start, observation("up")),
            (start + 360, observation("unknown")),
            (start + 540, observation("down")),
            (start + 480 * 180, observation("down")),
        ];
        let results = histories(&[id, other], start, &rounds, false);
        let (h, last, at, legacy) = &results[&id];
        assert_eq!(*legacy, 3);
        assert_eq!(h.len(), 480);
        assert_eq!(&h[..5], "u-?d-");
        assert_eq!(last.as_ref().unwrap().state, "down");
        assert_eq!(at.unwrap().timestamp(), start + 540);
        assert_eq!(results[&other].0, "-".repeat(480));
        assert_eq!(
            histories(&[id], start, &rounds, true)[&id].0,
            "-".repeat(480)
        );
        let mut dual = rounds;
        dual.push((start + 720, serde_json::json!([{"id":id,"state":"down","latency_ms":null,"detail":"","policy":3,"sites":[-1,-1,-1,-1,-1,-1],"link":{"state":"up","latency_ms":17,"detail":"TCP connected","kind":"tcp"}}])));
        assert_eq!(
            histories(&[id], start, &dual, false)[&id].0.as_bytes()[4],
            b'd'
        );
        assert_eq!(
            histories(&[id], start, &dual, true)[&id].0.as_bytes()[4],
            b'u'
        );
    }
}

pub async fn prune(pool: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    let proxy = sqlx::query("DELETE FROM proxy_lab_rounds WHERE slot<NOW()-INTERVAL '7 days'")
        .execute(pool)
        .await
        .map(|done| done.rows_affected())?;
    let local = sqlx::query("DELETE FROM local_network_rounds WHERE slot<NOW()-INTERVAL '7 days'")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(proxy + local)
}
