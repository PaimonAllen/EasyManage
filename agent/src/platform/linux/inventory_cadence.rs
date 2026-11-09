//! Independent cold-inventory clocks. Reused observations retain their original timestamps.
use super::inventory::ExtendedInventory;
use chrono::{DateTime, Utc};
use easymanage_protocol::{InventorySample, InventorySamples};

pub(super) const USERS_SECONDS: u64 = 180;
pub(super) const PROXY_SECONDS: u64 = 300;
pub(super) const AI_SECONDS: u64 = 300;
pub(super) const SOFTWARE_SECONDS: u64 = 3600;
pub(super) const PROGRAMMING_SECONDS: u64 = 43_200;

#[derive(Clone, Copy)]
pub(super) struct Plan {
    pub users: bool,
    pub proxy: bool,
    pub ai: bool,
    pub software: bool,
    pub programming: bool,
}

fn due(sample: Option<&InventorySample>, seconds: u64, now: DateTime<Utc>) -> bool {
    sample.is_none_or(|sample| {
        sample.interval_seconds != seconds
            || now < sample.collected_at
            || (now - sample.collected_at).num_milliseconds() >= (seconds * 1000 - 250) as i64
    })
}

impl Plan {
    pub fn all(users: bool, proxy: bool) -> Self {
        Self {
            users,
            proxy,
            ai: true,
            software: true,
            programming: true,
        }
    }

    pub fn due(previous: Option<&ExtendedInventory>, users: bool, proxy: bool) -> Self {
        let Some(previous) = previous else {
            return Self::all(users, proxy);
        };
        let now = Utc::now();
        let samples = &previous.samples;
        Self {
            users: users && due(samples.users.as_ref(), USERS_SECONDS, now),
            proxy: proxy && due(samples.proxy.as_ref(), PROXY_SECONDS, now),
            ai: due(samples.ai_tools.as_ref(), AI_SECONDS, now),
            software: due(samples.software.as_ref(), SOFTWARE_SECONDS, now),
            programming: due(samples.programming.as_ref(), PROGRAMMING_SECONDS, now),
        }
    }

    pub fn processes(self) -> bool {
        self.users || self.proxy || self.ai
    }

    fn refreshes_issue(self, component: &str) -> bool {
        match component {
            "users" | "sessions" => self.users,
            "proxy" => self.proxy,
            "ai_tools" => self.ai,
            "software" => self.software,
            "processes" => self.processes(),
            _ => true,
        }
    }

    pub fn merge(
        self,
        mut current: ExtendedInventory,
        previous: Option<ExtendedInventory>,
        users: bool,
        proxy: bool,
    ) -> ExtendedInventory {
        let mut samples = InventorySamples::default();
        if let Some(old) = previous {
            samples = old.samples;
            if users && !self.users {
                current.users = old.users;
            }
            if proxy && !self.proxy {
                current.proxy_services = old.proxy_services;
            }
            if !self.ai {
                current.ai_tools = old.ai_tools;
            }
            if !self.programming {
                current.programming_tools = old.programming_tools;
            }
            if !self.software {
                current.software_packages = old.software_packages;
                current.running_services = old.running_services;
            }
            current
                .issues
                .extend(old.issues.into_iter().filter(|issue| {
                    !self.refreshes_issue(&issue.component)
                        && (users || !matches!(issue.component.as_str(), "users" | "sessions"))
                        && (proxy || issue.component != "proxy")
                }));
        }
        let at = current.collected_at;
        let sample = |seconds| {
            Some(InventorySample {
                collected_at: at,
                interval_seconds: seconds,
            })
        };
        if self.users {
            samples.users = sample(USERS_SECONDS);
        }
        if self.proxy {
            samples.proxy = sample(PROXY_SECONDS);
        }
        if self.ai {
            samples.ai_tools = sample(AI_SECONDS);
        }
        if self.software {
            samples.software = sample(SOFTWARE_SECONDS);
        }
        if self.programming {
            samples.programming = sample(PROGRAMMING_SECONDS);
        }
        if !users {
            samples.users = None;
        }
        if !proxy {
            samples.proxy = None;
        }
        current.samples = samples;
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(at: DateTime<Utc>) -> ExtendedInventory {
        ExtendedInventory {
            collected_at: at,
            samples: Default::default(),
            privilege: easymanage_protocol::AgentPrivilege {
                mode: "basic".into(),
                username: "fixture".into(),
                uid: 1000,
            },
            users: vec![],
            block_devices: vec![],
            interfaces: vec![],
            overlay_networks: vec![],
            routes: vec![],
            proxy_services: vec![],
            software_packages: vec![],
            running_services: vec![],
            ai_tools: vec![],
            programming_tools: vec![],
            issues: vec![],
        }
    }

    #[test]
    fn network_refresh_preserves_cold_data_timestamps_and_disabled_scans_are_cleared() {
        let at = Utc::now() - chrono::Duration::seconds(60);
        let mut old = fixture(at);
        old.software_packages
            .push(easymanage_protocol::SoftwarePackage {
                name: "cached-package".into(),
                version: "1".into(),
            });
        old.issues.push(easymanage_protocol::CollectionIssue {
            component: "ai_tools".into(),
            message: "prior visibility limit".into(),
        });
        let mut old = Plan::all(true, true).merge(old, None, true, true);
        let due = Plan::due(Some(&old), true, true);
        assert!(!due.processes() && !due.software && !due.programming);
        let refreshed = due.merge(fixture(Utc::now()), Some(old.clone()), true, true);
        assert_eq!(refreshed.software_packages[0].name, "cached-package");
        assert_eq!(
            refreshed.samples.programming.as_ref().unwrap().collected_at,
            at
        );
        assert!(
            refreshed.collected_at > at
                && refreshed
                    .issues
                    .iter()
                    .any(|issue| issue.component == "ai_tools")
        );
        old.samples.users.as_mut().unwrap().collected_at =
            Utc::now() - chrono::Duration::seconds(181);
        let due = Plan::due(Some(&old), true, true);
        assert!(due.users && !due.ai && !due.proxy && !due.programming);
        let due = Plan::due(Some(&old), false, false);
        assert!(!due.users && !due.proxy);
        let disabled = due.merge(fixture(Utc::now()), Some(old), false, false);
        assert!(disabled.samples.users.is_none() && disabled.samples.proxy.is_none());
        assert_eq!(
            disabled.samples.programming.as_ref().unwrap().collected_at,
            at
        );
    }
    #[test]
    fn independent_boundaries_do_not_refresh_long_lived_observations() {
        let at = Utc::now();
        let sample = InventorySample {
            collected_at: at,
            interval_seconds: USERS_SECONDS,
        };
        assert!(!due(
            Some(&sample),
            USERS_SECONDS,
            at + chrono::Duration::seconds(179)
        ));
        assert!(due(
            Some(&sample),
            USERS_SECONDS,
            at + chrono::Duration::seconds(180)
        ));
        let programming = InventorySample {
            collected_at: at,
            interval_seconds: PROGRAMMING_SECONDS,
        };
        assert!(!due(
            Some(&programming),
            PROGRAMMING_SECONDS,
            at + chrono::Duration::hours(11)
        ));
        assert!(due(
            Some(&programming),
            PROGRAMMING_SECONDS,
            at + chrono::Duration::hours(12)
        ));
        assert!(due(
            Some(&programming),
            PROGRAMMING_SECONDS,
            at - chrono::Duration::seconds(1)
        ));
        assert!(due(None, SOFTWARE_SECONDS, at));
    }
}
