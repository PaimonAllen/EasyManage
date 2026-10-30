use super::inventory::{ExtendedInventory, issue};
use super::{hardware, mounts, network_capacity, permissions, temperature};
use crate::platform::common::gpu;
use crate::store::Store;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use easymanage_protocol::{
    CollectionPolicy, DiskMetric, GpuMetric, HardwareDetails, HostTemperatures, LiveMetrics,
    MetricsSnapshot, NetworkMetric, NodeInventory, PermissionAudit, PlatformStorageMetric,
};
use easymanage_runtime::process::ProcessSampler;
use easymanage_runtime::storage::{executable_bytes, sqlite_state_bytes};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use sysinfo::{DiskRefreshKind, Disks, Networks, System};

const GPU_FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(60);
const STORAGE_FOOTPRINT_INTERVAL: Duration = Duration::from_secs(3_600);

pub struct Collector {
    cached_temperatures: Option<HostTemperatures>,
    temperature_refreshed_at: Option<Instant>,
    policy: CollectionPolicy,
    cached_gpus: Vec<GpuMetric>,
    gpu_refreshed_at: Option<Instant>,
    gpu_collected_at: Option<DateTime<Utc>>,
    system: System,
    disks: Disks,
    networks: Networks,
    network_refreshed_at: Instant,
    network_list_refreshed_at: Instant,
    cached_networks: Vec<NetworkMetric>,
    last_fast: Option<LiveMetrics>,
    fast_refreshed_at: Instant,
    disk_totals: Option<HashMap<String, (u64, u64)>>,
    disk_refreshed_at: Instant,
    disk_collected_at: Option<DateTime<Utc>>,
    cached_disks: Vec<DiskMetric>,
    mount_inventory_failed: bool,
    network_capacity: network_capacity::Sampler,
    gpu_retry_at: Option<Instant>,
    gpu_issue: Option<String>,
    inventory: Option<NodeInventory>,
    inventory_refreshed_at: Instant,
    extended: ExtendedInventory,
    user_inventory_ready: bool,
    extended_refreshed_at: Instant,
    extended_task: Option<tokio::task::JoinHandle<ExtendedInventory>>,
    permission_audit: Option<PermissionAudit>,
    permission_refreshed_at: Instant,
    permission_task: Option<tokio::task::JoinHandle<PermissionAudit>>,
    process: ProcessSampler,
    state_database: PathBuf,
    cached_storage: Option<PlatformStorageMetric>,
    storage_refreshed_at: Option<Instant>,
    hardware: Option<HardwareDetails>,
    hardware_refreshed_at: Option<Instant>,
    store: Store,
}

impl Collector {
    pub async fn initialize(state_database: &Path, store: &Store) -> Self {
        let mut system = System::new();
        system.refresh_cpu_all();
        system.refresh_memory();
        let disks = Disks::new();
        let networks = Networks::new_with_refreshed_list();
        let network_refreshed_at = Instant::now();
        tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
        system.refresh_cpu_usage();
        // Read the persisted switch before any session/group probes, including startup.
        let users_enabled = store.users_enabled().await.unwrap_or(false);
        let proxy_enabled = store.proxy_enabled().await.unwrap_or(false);
        let extended = ExtendedInventory::collect(users_enabled, proxy_enabled).await;
        let permissions_enabled = store.permissions_enabled().await.unwrap_or(false);
        let permission_audit = if permissions_enabled {
            Some(permissions::collect(&extended.users).await)
        } else {
            None
        };
        let hardware = store
            .hardware_details(extended.privilege.uid)
            .await
            .ok()
            .flatten()
            .filter(|details| hardware::cache_age(details.collected_at, Utc::now()).is_some());
        let hardware_refreshed_at = hardware
            .as_ref()
            .and_then(|details| hardware::cache_age(details.collected_at, Utc::now()))
            .and_then(|age| Instant::now().checked_sub(age));

        let network_capacity =
            network_capacity::Sampler::load(state_database, extended.privilege.uid, store).await;
        Self {
            cached_temperatures: None,
            temperature_refreshed_at: None,
            policy: CollectionPolicy {
                users_enabled: Some(users_enabled),
                permissions_enabled: Some(permissions_enabled),
                proxy_enabled: Some(proxy_enabled),
                ..Default::default()
            },
            cached_gpus: Vec::new(),
            gpu_refreshed_at: None,
            gpu_collected_at: None,
            system,
            disks,
            networks,
            network_refreshed_at,
            network_list_refreshed_at: network_refreshed_at,
            cached_networks: Vec::new(),
            last_fast: None,
            fast_refreshed_at: Instant::now(),
            disk_totals: None,
            disk_refreshed_at: Instant::now(),
            disk_collected_at: None,
            cached_disks: Vec::new(),
            mount_inventory_failed: false,
            network_capacity,
            gpu_retry_at: None,
            gpu_issue: None,
            inventory: None,
            inventory_refreshed_at: Instant::now(),
            extended,
            user_inventory_ready: users_enabled,
            extended_refreshed_at: Instant::now(),
            extended_task: None,
            permission_audit,
            permission_refreshed_at: Instant::now(),
            permission_task: None,
            process: ProcessSampler::default(),
            state_database: state_database.to_path_buf(),
            cached_storage: None,
            storage_refreshed_at: None,
            hardware,
            hardware_refreshed_at,
            store: store.clone(),
        }
    }

    pub fn apply_policy(&mut self, policy: CollectionPolicy) {
        if policy.proxy_enabled.unwrap_or(true) != self.policy.proxy_enabled.unwrap_or(true) {
            // Never publish a previous-mode proxy scan, including after re-enable.
            if let Some(task) = self.extended_task.take() {
                task.abort();
            }
            self.extended.proxy_services.clear();
            self.extended
                .issues
                .retain(|issue| issue.component != "proxy");
            self.extended_refreshed_at = Instant::now()
                .checked_sub(Duration::from_secs(3601))
                .unwrap_or_else(Instant::now);
        }
        if policy.permissions_enabled.unwrap_or(true)
            != self.policy.permissions_enabled.unwrap_or(true)
        {
            if let Some(task) = self.permission_task.take() {
                task.abort();
            }
            self.permission_audit = None;
            self.permission_refreshed_at = Instant::now()
                .checked_sub(Duration::from_secs(3601))
                .unwrap_or_else(Instant::now);
        }
        if policy.users_enabled.unwrap_or(true) != self.policy.users_enabled.unwrap_or(true) {
            // Discard in-flight old-mode work, never publish its session evidence later.
            if let Some(task) = self.extended_task.take() {
                task.abort();
            }
            self.extended.clear_user_status();
            self.user_inventory_ready = false;
            self.extended_refreshed_at = Instant::now()
                .checked_sub(Duration::from_secs(3601))
                .unwrap_or_else(Instant::now);
        }
        self.policy = policy;
    }

    pub fn inventory(&mut self) -> Result<NodeInventory> {
        if let Some(inventory) = &self.inventory
            && self.inventory_refreshed_at.elapsed() + Duration::from_millis(250)
                < Duration::from_secs(self.policy.node_seconds)
        {
            return Ok(inventory.clone());
        }
        let cpu_model = self
            .system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().trim().to_owned())
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| "unknown".to_owned());

        let inventory = NodeInventory {
            hostname: System::host_name().context("could not determine the hostname")?,
            os_name: System::name().unwrap_or_else(|| std::env::consts::OS.to_owned()),
            os_version: System::os_version().unwrap_or_else(|| "unknown".to_owned()),
            kernel_version: System::kernel_version().unwrap_or_else(|| "unknown".to_owned()),
            architecture: System::cpu_arch(),
            cpu_model,
            cpu_logical_cores: self.system.cpus().len().try_into().unwrap_or(u32::MAX),
            cpu_physical_cores: System::physical_core_count()
                .and_then(|count| u32::try_from(count).ok()),
        };
        self.inventory = Some(inventory.clone());
        self.inventory_refreshed_at = Instant::now();
        Ok(inventory)
    }

    pub async fn collect(&mut self) -> MetricsSnapshot {
        let fast = self.collect_fast();
        // Read completed work only; never wait for a slow inventory scan on the
        // live sampler's path. At most one task per category can be in flight.
        if self
            .extended_task
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            if let Some(task) = self.extended_task.take() {
                match task.await {
                    Ok(extended) => {
                        self.extended = extended;
                        self.user_inventory_ready = self.policy.users_enabled.unwrap_or(true);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "inventory refresh failed; retaining prior snapshot")
                    }
                }
                self.extended_refreshed_at = Instant::now();
            }
        }
        if self.extended_refreshed_at.elapsed() + Duration::from_millis(250)
            >= Duration::from_secs(self.policy.extended_seconds)
            && self.extended_task.is_none()
        {
            self.extended_task = Some(tokio::spawn(ExtendedInventory::collect(
                self.policy.users_enabled.unwrap_or(true),
                self.policy.proxy_enabled.unwrap_or(true),
            )));
        }
        if self
            .permission_task
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            if let Some(task) = self.permission_task.take() {
                match task.await {
                    Ok(audit) => self.permission_audit = Some(audit),
                    Err(error) => {
                        tracing::warn!(%error, "permission refresh failed; retaining prior snapshot")
                    }
                }
                self.permission_refreshed_at = Instant::now();
            }
        }
        if self.policy.permissions_enabled.unwrap_or(true)
            && self.permission_refreshed_at.elapsed() + Duration::from_millis(250)
                >= Duration::from_secs(self.policy.permission_seconds)
            && self.permission_task.is_none()
        {
            let users = self.extended.users.clone();
            self.permission_task = Some(tokio::spawn(
                async move { permissions::collect(&users).await },
            ));
        }
        self.collect_disks();
        self.network_capacity
            .refresh(&mut self.cached_disks, &self.store)
            .await;
        if self.temperature_refreshed_at.is_none_or(|at| {
            at.elapsed() + Duration::from_millis(250)
                >= Duration::from_secs(self.policy.disk_seconds.max(30))
        }) {
            self.cached_temperatures = Some(temperature::collect());
            self.temperature_refreshed_at = Some(Instant::now());
        }

        let load = System::load_average();
        if self.gpu_refreshed_at.is_none_or(|at| {
            at.elapsed() + Duration::from_millis(250)
                >= Duration::from_secs(self.policy.gpu_seconds)
        }) && self
            .gpu_retry_at
            .is_none_or(|retry_at| Instant::now() >= retry_at)
        {
            let (gpus, issue) = gpu::collect().await;
            self.gpu_retry_at = if gpus.is_empty() {
                Some(Instant::now() + GPU_FAILURE_RETRY_INTERVAL)
            } else {
                None
            };
            self.gpu_issue.clone_from(&issue);
            self.cached_gpus = gpus;
            self.gpu_refreshed_at = Some(Instant::now());
            self.gpu_collected_at = Some(Utc::now());
        }
        let mut collection_issues = self.extended.issues.clone();
        if self.mount_inventory_failed {
            collection_issues.push(issue(
                "disk",
                "挂载清单不可读取；网络挂载及共享关系可能不完整",
            ));
        }
        if let Some(message) = &self.gpu_issue {
            collection_issues.push(issue("gpu", &message));
        }
        if self
            .hardware_refreshed_at
            .is_none_or(|at| at.elapsed() >= hardware::INTERVAL)
        {
            let inventory = self.inventory().ok();
            let memory_total = self.system.total_memory();
            let gpus = self.cached_gpus.clone();
            let disks = self.extended.block_devices.clone();
            match tokio::task::spawn_blocking(move || {
                hardware::collect(inventory, memory_total, &gpus, &disks)
            })
            .await
            {
                Ok(details) => {
                    if let Err(error) = self
                        .store
                        .save_hardware_details(self.extended.privilege.uid, &details)
                        .await
                    {
                        tracing::warn!(%error, "hardware cache could not be persisted");
                    }
                    self.hardware = Some(details);
                }
                Err(error) => {
                    tracing::warn!(%error, "hardware inventory failed; retaining prior snapshot")
                }
            }
            self.hardware_refreshed_at = Some(Instant::now());
        }
        if self
            .storage_refreshed_at
            .is_none_or(|at| at.elapsed() >= STORAGE_FOOTPRINT_INTERVAL)
        {
            let binary_bytes = executable_bytes();
            let state_bytes = sqlite_state_bytes(&self.state_database);
            self.cached_storage = Some(PlatformStorageMetric {
                collected_at: Utc::now(),
                binary_bytes,
                state_bytes,
                web_bytes: None,
                database_bytes: None,
                total_bytes: binary_bytes
                    .zip(state_bytes)
                    .and_then(|(binary, state)| binary.checked_add(state)),
            });
            self.storage_refreshed_at = Some(Instant::now());
        }
        MetricsSnapshot {
            agent_control: None,
            network_activity: None,
            hardware_details: self.hardware.clone(),
            temperatures: self.cached_temperatures.clone(),
            collection_policy: Some(self.policy.clone()),
            fast_collected_at: Some(fast.collected_at),
            gpu_collected_at: self.gpu_collected_at,
            collected_at: Utc::now(),
            cpu_usage_percent: fast.cpu_usage_percent,
            load_average_one: load.one,
            load_average_five: load.five,
            load_average_fifteen: load.fifteen,
            memory_total_bytes: fast.memory_total_bytes,
            memory_used_bytes: fast.memory_used_bytes,
            swap_total_bytes: self.system.total_swap(),
            swap_used_bytes: self.system.used_swap(),
            uptime_seconds: System::uptime(),
            disks: self.cached_disks.clone(),
            block_devices: self.extended.block_devices.clone(),
            disk_collected_at: self.disk_collected_at,
            networks: self.cached_networks.clone(),
            gpus: self.cached_gpus.clone(),
            platform_process: Some(self.process.sample()),
            platform_storage: self.cached_storage.clone(),
            privilege: Some(self.extended.privilege.clone()),
            users: if self.policy.users_enabled.unwrap_or(true) && self.user_inventory_ready {
                self.extended.users.clone()
            } else {
                Vec::new()
            },
            interfaces: self.extended.interfaces.clone(),
            overlay_networks: self.extended.overlay_networks.clone(),
            routes: self.extended.routes.clone(),
            proxy_services: self.extended.proxy_services.clone(),
            collection_issues,
            inventory_collected_at: Some(self.extended.collected_at),
            software_packages: self.extended.software_packages.clone(),
            running_services: self.extended.running_services.clone(),
            host_security: Some(self.extended.host_security.clone()),
            permission_audit: self.permission_audit.clone(),
            ai_tools: self.extended.ai_tools.clone(),
            programming_tools: self.extended.programming_tools.clone(),
        }
    }

    pub fn collect_fast(&mut self) -> LiveMetrics {
        if let Some(sample) = &self.last_fast
            && self.fast_refreshed_at.elapsed()
                < Duration::from_millis(self.policy.live_seconds * 1000 - 250)
        {
            return sample.clone();
        }
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        let elapsed = self.network_refreshed_at.elapsed().as_secs_f64().max(0.001);
        // Rates remain real-time; discovering added/removed NICs belongs to the
        // slower inventory cadence instead of traversing the list every second.
        let refresh_list = self.network_list_refreshed_at.elapsed()
            >= Duration::from_secs(self.policy.extended_seconds);
        self.networks.refresh(refresh_list);
        if refresh_list {
            self.network_list_refreshed_at = Instant::now();
        }
        let refreshed_at = Instant::now();
        self.cached_networks = self
            .networks
            .iter()
            .map(|(name, network)| NetworkMetric {
                interface: name.clone(),
                received_bytes_per_second: bytes_per_second(network.received(), elapsed),
                transmitted_bytes_per_second: bytes_per_second(network.transmitted(), elapsed),
                total_received_bytes: network.total_received(),
                total_transmitted_bytes: network.total_transmitted(),
            })
            .collect();
        self.network_refreshed_at = refreshed_at;
        self.fast_refreshed_at = refreshed_at;
        let sample = LiveMetrics {
            collected_at: Utc::now(),
            cpu_usage_percent: self.system.global_cpu_usage(),
            memory_total_bytes: self.system.total_memory(),
            memory_used_bytes: self.system.used_memory(),
            network_received_bytes_per_second: self
                .cached_networks
                .iter()
                .map(|network| network.received_bytes_per_second)
                .sum(),
            network_transmitted_bytes_per_second: self
                .cached_networks
                .iter()
                .map(|network| network.transmitted_bytes_per_second)
                .sum(),
        };
        self.last_fast = Some(sample.clone());
        sample
    }

    fn collect_disks(&mut self) {
        if self.disk_collected_at.is_none()
            || self.disk_refreshed_at.elapsed() + Duration::from_millis(250)
                >= Duration::from_secs(self.policy.disk_seconds)
        {
            // Discover without statvfs; sysinfo only excludes a subset of
            // network filesystem types by default (e.g. not SSHFS or Ceph).
            self.disks
                .refresh_specifics(true, DiskRefreshKind::nothing());
            for disk in self.disks.list_mut() {
                if !mounts::is_network_filesystem(&disk.file_system().to_string_lossy())
                    && disk.file_system() != "autofs"
                {
                    disk.refresh_specifics(DiskRefreshKind::nothing().with_storage());
                }
            }
            let disk_totals = read_disk_totals();
            let disk_refreshed_at = Instant::now();
            let disk_elapsed = disk_refreshed_at
                .duration_since(self.disk_refreshed_at)
                .as_secs_f64();
            self.cached_disks = self
                .disks
                .iter()
                .map(|disk| {
                    let key = disk_counter_key(disk.name());
                    let current = key.as_ref().and_then(|key| disk_totals.as_ref()?.get(key));
                    let previous = key
                        .as_ref()
                        .and_then(|key| self.disk_totals.as_ref()?.get(key));
                    let (read_rate, write_rate) = disk_rates(previous, current, disk_elapsed);
                    DiskMetric {
                        mount: None,
                        name: disk.name().to_string_lossy().into_owned(),
                        mount_point: disk.mount_point().to_string_lossy().into_owned(),
                        file_system: disk.file_system().to_string_lossy().into_owned(),
                        total_bytes: disk.total_space(),
                        available_bytes: disk.available_space(),
                        // Several mount points can share a device counter. New/unreadable/reset devices
                        // deliberately have no rate until two valid consecutive baselines exist.
                        read_bytes_per_second: read_rate,
                        written_bytes_per_second: write_rate,
                    }
                })
                .collect();
            match mounts::read() {
                Ok(inventory) => {
                    mounts::enrich(&mut self.cached_disks, inventory);
                    self.mount_inventory_failed = false;
                }
                Err(_) => self.mount_inventory_failed = true,
            }
            self.disk_totals = disk_totals;
            self.disk_refreshed_at = disk_refreshed_at;
            self.disk_collected_at = Some(Utc::now());
        }
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        if let Some(task) = &self.extended_task {
            task.abort();
        }
        if let Some(task) = &self.permission_task {
            task.abort();
        }
    }
}

fn disk_counter_key(device: &std::ffi::OsStr) -> Option<String> {
    let path = Path::new(device);
    if !path.starts_with("/dev") {
        return None;
    }
    let resolved = std::fs::canonicalize(path).ok()?;
    resolved.file_name()?.to_str().map(str::to_owned)
}

fn read_disk_totals() -> Option<HashMap<String, (u64, u64)>> {
    let mut contents = String::new();
    std::fs::File::open("/proc/diskstats")
        .ok()?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut contents)
        .ok()?;
    if contents.len() > 1024 * 1024 {
        return None;
    }
    Some(
        contents
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                let name = (*fields.get(2)?).to_owned();
                // Linux diskstats sectors are always 512 bytes, independent of the device block size.
                let read = fields.get(5)?.parse::<u64>().ok()?.checked_mul(512)?;
                let written = fields.get(9)?.parse::<u64>().ok()?.checked_mul(512)?;
                Some((name, (read, written)))
            })
            .collect(),
    )
}

fn disk_rates(
    previous: Option<&(u64, u64)>,
    current: Option<&(u64, u64)>,
    elapsed: f64,
) -> (Option<f64>, Option<f64>) {
    let (Some(previous), Some(current)) = (previous, current) else {
        return (None, None);
    };
    if !elapsed.is_finite() || elapsed <= 0.0 {
        return (None, None);
    }
    let (Some(read), Some(written)) = (
        current.0.checked_sub(previous.0),
        current.1.checked_sub(previous.1),
    ) else {
        return (None, None);
    };
    (
        Some(bytes_per_second(read, elapsed)),
        Some(bytes_per_second(written, elapsed)),
    )
}

#[allow(clippy::cast_precision_loss)]
fn bytes_per_second(bytes: u64, elapsed_seconds: f64) -> f64 {
    bytes as f64 / elapsed_seconds
}

#[cfg(test)]
mod tests {
    use super::disk_rates;

    #[tokio::test]
    async fn pending_slow_scans_do_not_block_reports_or_spawn_duplicates() {
        use super::Collector;
        use std::time::{Duration, Instant};
        let root = std::env::temp_dir().join(format!(
            "easymanage-collector-test.{}",
            uuid::Uuid::new_v4()
        ));
        let store = crate::store::Store::open(&root.join("state.db"))
            .await
            .unwrap();
        let mut collector = Collector::initialize(&root.join("state.db"), &store).await;
        collector.extended_refreshed_at = Instant::now() - Duration::from_secs(3601);
        collector.permission_refreshed_at = Instant::now() - Duration::from_secs(3601);
        collector.extended_task = Some(tokio::spawn(std::future::pending()));
        collector.permission_task = Some(tokio::spawn(std::future::pending()));
        let inventory_task = collector.extended_task.as_ref().unwrap().id();
        let permission_task = collector.permission_task.as_ref().unwrap().id();
        let inventory_at = collector.extended.collected_at;
        let permissions_at = collector.permission_audit.as_ref().unwrap().collected_at;
        // Skip other slow first-use probes: this regression isolates the two tasks.
        collector.gpu_refreshed_at = Some(Instant::now());
        collector.disk_collected_at = Some(chrono::Utc::now());
        collector.temperature_refreshed_at = Some(Instant::now());
        collector.hardware_refreshed_at = Some(Instant::now());
        for _ in 0..3 {
            let snapshot = tokio::time::timeout(Duration::from_millis(500), collector.collect())
                .await
                .expect("a pending slow scan must not block a report");
            assert_eq!(snapshot.inventory_collected_at, Some(inventory_at));
            assert_eq!(
                snapshot.permission_audit.unwrap().collected_at,
                permissions_at
            );
            assert_eq!(
                collector.extended_task.as_ref().unwrap().id(),
                inventory_task
            );
            assert_eq!(
                collector.permission_task.as_ref().unwrap().id(),
                permission_task
            );
        }
        let inventory_abort = collector.extended_task.as_ref().unwrap().abort_handle();
        let permission_abort = collector.permission_task.as_ref().unwrap().abort_handle();
        collector.apply_policy(easymanage_protocol::CollectionPolicy {
            users_enabled: Some(false),
            ..Default::default()
        });
        tokio::task::yield_now().await;
        assert!(
            inventory_abort.is_finished(),
            "old-mode inventory task is cancelled immediately"
        );
        assert!(
            !permission_abort.is_finished(),
            "separate permission checks remain running"
        );
        // Hold a disabled-mode scan pending; reports must not expose old evidence.
        collector.extended_task = Some(tokio::spawn(std::future::pending()));
        let snapshot = collector.collect().await;
        assert!(snapshot.users.is_empty());
        assert_eq!(
            snapshot.collection_policy.unwrap().users_enabled,
            Some(false)
        );
        collector.apply_policy(easymanage_protocol::CollectionPolicy {
            users_enabled: Some(true),
            ..Default::default()
        });
        assert!(
            !collector.user_inventory_ready,
            "re-enable waits for a new scan instead of recycling old status"
        );
        collector.apply_policy(easymanage_protocol::CollectionPolicy {
            users_enabled: Some(true),
            permissions_enabled: Some(false),
            ..Default::default()
        });
        tokio::task::yield_now().await;
        assert!(
            permission_abort.is_finished(),
            "disabling permission collection cancels the pending task"
        );
        assert!(collector.permission_audit.is_none());
        let snapshot = collector.collect().await;
        assert!(snapshot.permission_audit.is_none());
        assert!(
            collector.permission_task.is_none(),
            "disabled permission checks never schedule new scans"
        );
        collector.apply_policy(easymanage_protocol::CollectionPolicy {
            users_enabled: Some(true),
            permissions_enabled: Some(true),
            ..Default::default()
        });
        assert!(
            collector.permission_audit.is_none(),
            "re-enable must not reuse old evidence"
        );
        collector.collect().await;
        assert!(
            collector.permission_task.is_some(),
            "re-enable schedules a fresh check"
        );
        if let Some(task) = collector.extended_task.take() {
            task.abort();
        }
        collector.extended_task = Some(tokio::spawn(std::future::pending()));
        let proxy_abort = collector.extended_task.as_ref().unwrap().abort_handle();
        let users_before = collector.extended.users.len();
        collector
            .extended
            .proxy_services
            .push(easymanage_protocol::ProxyService {
                name: "mihomo".into(),
                pid: 123,
                uid: 1000,
                username: "fixture".into(),
                listen_addresses: vec!["tcp://127.0.0.1:12345".into()],
            });
        let mut policy = collector.policy.clone();
        policy.proxy_enabled = Some(false);
        collector.apply_policy(policy.clone());
        tokio::task::yield_now().await;
        assert!(
            proxy_abort.is_finished(),
            "proxy mode change cancels old shared inventory work"
        );
        assert!(collector.extended.proxy_services.is_empty());
        assert_eq!(
            collector.extended.users.len(),
            users_before,
            "proxy switch does not clear user observations"
        );
        assert!(
            collector.permission_task.is_some(),
            "independent permission work remains scheduled"
        );
        collector.extended_task = Some(tokio::spawn(std::future::pending()));
        let snapshot = collector.collect().await;
        assert!(snapshot.proxy_services.is_empty());
        assert_eq!(
            snapshot.collection_policy.unwrap().proxy_enabled,
            Some(false)
        );
        policy.proxy_enabled = Some(true);
        collector.apply_policy(policy);
        assert!(
            collector.extended.proxy_services.is_empty(),
            "re-enable never restores old proxy evidence"
        );
        drop(collector);
        tokio::task::yield_now().await;
        assert!(inventory_abort.is_finished() && permission_abort.is_finished());
        drop(store);
        // Only this freshly-created test's private directory can be removed.
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disk_rates_require_two_valid_monotonic_observations() {
        assert_eq!(
            disk_rates(None, Some(&(10_000_000, 50_000_000)), 10.0),
            (None, None)
        );
        assert_eq!(disk_rates(Some(&(100, 200)), None, 10.0), (None, None));
        assert_eq!(
            disk_rates(Some(&(100, 200)), Some(&(99, 300)), 10.0),
            (None, None)
        );
        assert_eq!(
            disk_rates(Some(&(100, 200)), Some(&(200, 400)), 10.0),
            (Some(10.0), Some(20.0))
        );
        assert_eq!(
            disk_rates(Some(&(100, 200)), Some(&(100, 200)), 10.0),
            (Some(0.0), Some(0.0))
        );
    }
}
