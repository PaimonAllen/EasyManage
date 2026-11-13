//! Windows native metrics and bounded pwsh inventory. Slow observations never
//! block the one-second lane; failed sections preserve their original timestamps.
pub(crate) mod paths;
use crate::{platform::PlatformCollector, store::Store};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use easymanage_protocol::*;
use easymanage_runtime::{
    process::ProcessSampler,
    storage::{executable_bytes, sqlite_state_bytes},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use sysinfo::{DiskRefreshKind, Disks, Networks, System};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
};
const SCRIPT: &str = include_str!("inventory.ps1");
async fn powershell(request: Value) -> Result<Value> {
    let pwsh = tokio::task::spawn_blocking(easymanage_runtime::shell::resolve_pwsh).await??;
    let mut child = tokio::process::Command::new(pwsh)
    .args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "& ([scriptblock]::Create([Console]::In.ReadToEnd()))",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true)
    .spawn()
    .context("PowerShell 7 (pwsh.exe) is required in PATH")?;
    let script = format!(
        "& {{\n$request={} | ConvertFrom-Json\n{}\n}}\n",
        quote(&request.to_string()),
        SCRIPT
    );
    let mut input = child.stdin.take().context("missing pwsh stdin")?;
    let stdout = child.stdout.take().context("missing pwsh stdout")?;
    let result = tokio::time::timeout(Duration::from_secs(25), async {
        input.write_all(script.as_bytes()).await?;
        input.shutdown().await?;
        drop(input);
        let mut bytes = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        anyhow::ensure!(
            bytes.len() <= 4 * 1024 * 1024,
            "pwsh inventory exceeded output limit"
        );
        anyhow::ensure!(child.wait().await?.success(), "pwsh inventory failed");
        Ok::<Value, anyhow::Error>(serde_json::from_slice(&bytes)?)
    })
    .await;
    match result {
        Ok(value) => value,
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("pwsh inventory timed out")
        }
    }
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
fn canonical<T: serde::de::DeserializeOwned + serde::Serialize>(value: Value) -> Result<Value> {
    Ok(serde_json::to_value(serde_json::from_value::<T>(value)?)?)
}
fn checked_section(section: &str, value: Value) -> Result<Value> {
    match section {
        "users" => canonical::<Vec<UserStatus>>(value),
        "proxy" => canonical::<Vec<ProxyService>>(value),
        "hardware" => canonical::<HardwareDetails>(value),
        "permissions" => canonical::<PermissionAudit>(value),
        "network" => Ok(
            json!({"interfaces":canonical::<Vec<NetworkInterface>>(value["interfaces"].clone())?,"routes":canonical::<Vec<RouteInfo>>(value["routes"].clone())?}),
        ),
        "storage" => Ok(
            json!({"block_devices":canonical::<Vec<BlockDevice>>(value["block_devices"].clone())?}),
        ),
        "software" => Ok(
            json!({"software_packages":canonical::<Vec<SoftwarePackage>>(value["software_packages"].clone())?,"running_services":canonical::<Vec<ServiceStatus>>(value["running_services"].clone())?}),
        ),
        "programming" => Ok(
            json!({"programming_tools":canonical::<Vec<ProgrammingToolStatus>>(value["programming_tools"].clone())?}),
        ),
        "ai_tools" => {
            Ok(json!({"ai_tools":canonical::<Vec<AiToolStatus>>(value["ai_tools"].clone())?}))
        }
        _ => anyhow::bail!("unknown inventory section"),
    }
}
struct InventoryResult {
    requested: Vec<&'static str>,
    value: Result<Value>,
}
pub(crate) struct NativeCollector {
    system: System,
    networks: Networks,
    totals: HashMap<String, (u64, u64)>,
    network_at: Instant,
    cached_networks: Vec<NetworkMetric>,
    last_fast: Option<LiveMetrics>,
    policy: CollectionPolicy,
    privilege: AgentPrivilege,
    inventory: NodeInventory,
    values: Value,
    timestamps: HashMap<&'static str, (Instant, DateTime<Utc>)>,
    inventory_task: Option<JoinHandle<InventoryResult>>,
    retry_at: Instant,
    gpu_task: Option<JoinHandle<(Vec<GpuMetric>, Option<String>)>>,
    gpus: Vec<GpuMetric>,
    gpu_at: Option<(Instant, DateTime<Utc>)>,
    gpu_retry: Instant,
    disk_task: Option<JoinHandle<(Vec<DiskMetric>, Value)>>,
    overlay_task: Option<JoinHandle<Vec<OverlayNetworkStatus>>>,
    overlay_at: Option<Instant>,
    disk_totals: HashMap<String, (u64, u64, u64, u64)>,
    disks: Vec<DiskMetric>,
    disk_at: Option<(Instant, DateTime<Utc>)>,
    issues: Vec<CollectionIssue>,
    process: ProcessSampler,
    state_database: PathBuf,
    storage: Option<PlatformStorageMetric>,
    storage_at: Option<Instant>,
}
impl Drop for NativeCollector {
    fn drop(&mut self) {
        if let Some(t) = &self.inventory_task {
            t.abort()
        }
        if let Some(t) = &self.gpu_task {
            t.abort()
        }
        if let Some(t) = &self.overlay_task {
            t.abort();
        }
        if let Some(t) = &self.disk_task {
            t.abort()
        }
    }
}
impl PlatformCollector for NativeCollector {
    async fn initialize(state_database: &Path, _store: &Store) -> Result<Self> {
        let identity = powershell(json!({"identity":true})).await?;
        let identity = &identity["identity"];
        let native: NativeIdentity =
            serde_json::from_value(identity.clone()).context("Windows identity unavailable")?;
        let privilege = AgentPrivilege {
            mode: if native.administrator {
                "full"
            } else {
                "basic"
            }
            .into(),
            username: identity["username"]
                .as_str()
                .context("missing Windows username")?
                .into(),
            uid: u32::MAX,
            native: Some(native),
        };
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
        system.refresh_cpu_usage();
        let inventory = NodeInventory {
            hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
            os_name: System::name().unwrap_or_else(|| "Windows".into()),
            os_version: System::long_os_version().unwrap_or_default(),
            kernel_version: System::kernel_version().unwrap_or_default(),
            architecture: std::env::consts::ARCH.into(),
            cpu_model: system
                .cpus()
                .first()
                .map(|c| c.brand().to_owned())
                .unwrap_or_default(),
            cpu_logical_cores: system.cpus().len() as u32,
            cpu_physical_cores: System::physical_core_count().map(|n| n as u32),
        };
        Ok(Self {
            system,
            networks: Networks::new_with_refreshed_list(),
            totals: HashMap::new(),
            network_at: Instant::now(),
            cached_networks: vec![],
            last_fast: None,
            policy: CollectionPolicy::default(),
            privilege,
            inventory,
            values: json!({}),
            timestamps: HashMap::new(),
            inventory_task: None,
            retry_at: Instant::now(),
            gpu_task: None,
            gpus: vec![],
            gpu_at: None,
            gpu_retry: Instant::now(),
            overlay_task: None,
            overlay_at: None,
            disk_totals: HashMap::new(),
            disk_task: None,
            disks: vec![],
            disk_at: None,
            issues: vec![],
            process: ProcessSampler::default(),
            state_database: state_database.into(),
            storage: None,
            storage_at: None,
        })
    }
    fn apply_policy(&mut self, policy: CollectionPolicy) {
        // Drop observations immediately when an operator disables a category.
        for (enabled, section, fields) in [
            (policy.users_enabled.unwrap_or(true), "users", vec!["users"]),
            (
                policy.proxy_enabled.unwrap_or(true),
                "proxy",
                vec!["proxy_services"],
            ),
            (
                policy.permissions_enabled.unwrap_or(true),
                "permissions",
                vec!["permission_audit"],
            ),
        ] {
            if !enabled {
                for field in fields {
                    self.values.as_object_mut().unwrap().remove(field);
                }
                self.timestamps.remove(section);
            }
        }
        if self.policy.users_enabled != policy.users_enabled
            || self.policy.proxy_enabled != policy.proxy_enabled
            || self.policy.permissions_enabled != policy.permissions_enabled
        {
            if let Some(task) = self.inventory_task.take() {
                task.abort();
            }
        }
        self.policy = policy;
    }
    fn inventory(&mut self) -> Result<NodeInventory> {
        Ok(self.inventory.clone())
    }
    fn collect_fast(&mut self) -> LiveMetrics {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        let now = Instant::now();
        let elapsed = now.duration_since(self.network_at).as_secs_f64().max(0.001);
        self.network_at = now;
        let mut receive = 0.0;
        let mut transmit = 0.0;
        self.cached_networks.clear();
        for (name, net) in &self.networks {
            if name.to_lowercase().contains("loopback") {
                continue;
            }
            let total = (net.total_received(), net.total_transmitted());
            let old = self.totals.insert(name.clone(), total).unwrap_or(total);
            let rx = total.0.saturating_sub(old.0) as f64 / elapsed;
            let tx = total.1.saturating_sub(old.1) as f64 / elapsed;
            receive += rx;
            transmit += tx;
            self.cached_networks.push(NetworkMetric {
                interface: name.clone(),
                received_bytes_per_second: rx,
                transmitted_bytes_per_second: tx,
                total_received_bytes: total.0,
                total_transmitted_bytes: total.1,
            });
        }
        self.totals
            .retain(|name, _| self.networks.contains_key(name));
        let sample = LiveMetrics {
            collected_at: Utc::now(),
            cpu_usage_percent: self.system.global_cpu_usage(),
            memory_total_bytes: self.system.total_memory(),
            memory_used_bytes: self.system.used_memory(),
            network_received_bytes_per_second: receive,
            network_transmitted_bytes_per_second: transmit,
        };
        self.last_fast = Some(sample.clone());
        sample
    }
    async fn collect(&mut self) -> MetricsSnapshot {
        if self
            .inventory_task
            .as_ref()
            .is_some_and(|t| t.is_finished())
        {
            if let Some(task) = self.inventory_task.take() {
                match task.await {
                    Ok(InventoryResult {
                        requested,
                        value: Ok(value),
                    }) => {
                        self.issues.retain(|issue| {
                            !requested.contains(&issue.component.as_str())
                                && issue.component != "windows_inventory"
                        });
                        for section in requested {
                            if let Some(raw) = value.get(section) {
                                let result = match checked_section(section, raw.clone()) {
                                    Ok(result) => result,
                                    Err(_) => {
                                        self.issues.push(CollectionIssue{component:section.into(),message:"Windows inventory format unavailable; last valid observation retained".into()});
                                        continue;
                                    }
                                };
                                match section {
                                    "users" => self.values["users"] = result.clone(),
                                    "proxy" => self.values["proxy_services"] = result.clone(),
                                    "hardware" => self.values["hardware_details"] = result.clone(),
                                    "permissions" => {
                                        self.values["permission_audit"] = result.clone()
                                    }
                                    _ => {
                                        if let Some(fields) = result.as_object() {
                                            for (key, item) in fields {
                                                self.values[key] = item.clone();
                                            }
                                        }
                                    }
                                }
                                self.timestamps
                                    .insert(section, (Instant::now(), Utc::now()));
                            }
                        }
                        if let Ok(issues) =
                            serde_json::from_value::<Vec<CollectionIssue>>(value["issues"].clone())
                        {
                            self.issues.extend(issues);
                        }
                    }
                    _ => {
                        self.issues.retain(|i| i.component != "windows_inventory");
                        self.issues.push(CollectionIssue {
                            component: "windows_inventory".into(),
                            message: "pwsh inventory unavailable; cached timestamps retained"
                                .into(),
                        });
                        self.retry_at = Instant::now() + Duration::from_secs(60);
                    }
                }
            }
        }
        if self.inventory_task.is_none() && Instant::now() >= self.retry_at {
            let mut request = json!({"base":self.state_database.parent().and_then(Path::parent).map(|p|p.to_string_lossy().into_owned())});
            let mut requested = vec![];
            for (name, seconds, enabled) in [
                ("users", 180, self.policy.users_enabled.unwrap_or(true)),
                ("network", self.policy.extended_seconds, true),
                ("software", 3600, true),
                ("storage", 3600, true),
                ("programming", 43200, true),
                ("ai_tools", 300, true),
                ("proxy", 300, self.policy.proxy_enabled.unwrap_or(true)),
                ("hardware", 86400, true),
                (
                    "permissions",
                    self.policy.permission_seconds,
                    self.policy.permissions_enabled.unwrap_or(true),
                ),
            ] {
                if enabled
                    && self
                        .timestamps
                        .get(name)
                        .is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(seconds))
                {
                    request[name] = true.into();
                    requested.push(name);
                }
            }
            if !requested.is_empty() {
                self.inventory_task = Some(tokio::spawn(async move {
                    InventoryResult {
                        requested,
                        value: powershell(request).await,
                    }
                }));
                self.retry_at = Instant::now() + Duration::from_secs(60);
            }
        }
        if self.gpu_task.as_ref().is_some_and(|t| t.is_finished()) {
            if let Some(task) = self.gpu_task.take() {
                if let Ok((gpus, issue)) = task.await {
                    self.issues.retain(|i| i.component != "gpu");
                    if let Some(message) = issue {
                        self.issues.push(CollectionIssue {
                            component: "gpu".into(),
                            message,
                        });
                        self.gpu_retry = Instant::now() + Duration::from_secs(60);
                    } else {
                        self.gpus = gpus;
                        self.gpu_at = Some((Instant::now(), Utc::now()));
                    }
                }
            }
        }
        if self.gpu_task.is_none()
            && Instant::now() >= self.gpu_retry
            && self
                .gpu_at
                .is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(self.policy.gpu_seconds))
        {
            self.gpu_task = Some(tokio::spawn(async {
                crate::platform::common::gpu::Sampler::default()
                    .collect()
                    .await
            }));
        }
        if self.overlay_task.as_ref().is_some_and(|t| t.is_finished()) {
            if let Some(task) = self.overlay_task.take() {
                if let Ok(status) = task.await {
                    self.values["overlay_networks"] = serde_json::to_value(status).unwrap();
                    self.overlay_at = Some(Instant::now());
                }
            }
        }
        if self.overlay_task.is_none()
            && self.overlay_at.is_none_or(|at| {
                at.elapsed() >= Duration::from_secs(self.policy.extended_seconds.max(60))
            })
        {
            self.overlay_task = Some(tokio::spawn(async {
                let tail = std::env::var_os("ProgramFiles")
                    .map(PathBuf::from)
                    .map(|p| p.join("Tailscale/tailscale.exe"))
                    .filter(|p| p.is_file())
                    .unwrap_or_else(|| "tailscale.exe".into());
                let zero = std::env::var_os("ProgramFiles(x86)")
                    .or_else(|| std::env::var_os("ProgramFiles"))
                    .map(PathBuf::from)
                    .map(|p| p.join("ZeroTier/One/zerotier-one_x64.exe"))
                    .filter(|p| p.is_file());
                if let Some(zero) = zero {
                    crate::platform::common::overlay_networks::collect_with(
                        &tail.to_string_lossy(),
                        &zero.to_string_lossy(),
                        &["-q"],
                    )
                    .await
                } else {
                    crate::platform::common::overlay_networks::collect_with(
                        &tail.to_string_lossy(),
                        "zerotier-cli.exe",
                        &[],
                    )
                    .await
                }
            }));
        }
        if self.disk_task.as_ref().is_some_and(|t| t.is_finished()) {
            if let Some(task) = self.disk_task.take() {
                if let Ok((mut disks, io)) = task.await {
                    if let Some(rows) = io["diskio"].as_array() {
                        for row in rows {
                            let Some(name) = row["name"].as_str() else {
                                continue;
                            };
                            let Some((read, written, at, frequency)) = row["read"]
                                .as_u64()
                                .zip(row["written"].as_u64())
                                .zip(row["timestamp"].as_u64())
                                .zip(row["frequency"].as_u64())
                                .map(|(((r, w), t), f)| (r, w, t, f))
                            else {
                                continue;
                            };
                            if let Some((old_read, old_written, old_at, old_frequency)) = self
                                .disk_totals
                                .insert(name.into(), (read, written, at, frequency))
                            {
                                if frequency > 0
                                    && frequency == old_frequency
                                    && at > old_at
                                    && read >= old_read
                                    && written >= old_written
                                {
                                    let elapsed = (at - old_at) as f64 / frequency as f64;
                                    for disk in &mut disks {
                                        if disk
                                            .mount_point
                                            .trim_end_matches(['\\', '/'])
                                            .eq_ignore_ascii_case(name)
                                        {
                                            disk.read_bytes_per_second =
                                                Some((read - old_read) as f64 / elapsed);
                                            disk.written_bytes_per_second =
                                                Some((written - old_written) as f64 / elapsed);
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        self.disk_totals.clear();
                    }
                    self.disks = disks;
                    self.disk_at = Some((Instant::now(), Utc::now()));
                }
            }
        }
        if self.disk_task.is_none()
            && self
                .disk_at
                .is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(self.policy.disk_seconds))
        {
            self.disk_task = Some(tokio::spawn(async {
                let disks = tokio::task::spawn_blocking(|| {
                    Disks::new_with_refreshed_list_specifics(
                        DiskRefreshKind::nothing().with_storage(),
                    )
                    .list()
                    .iter()
                    .map(|disk| DiskMetric {
                        name: disk.name().to_string_lossy().into_owned(),
                        mount_point: disk.mount_point().to_string_lossy().into_owned(),
                        file_system: disk.file_system().to_string_lossy().into_owned(),
                        total_bytes: disk.total_space(),
                        available_bytes: disk.available_space(),
                        read_bytes_per_second: None,
                        written_bytes_per_second: None,
                        mount: None,
                    })
                    .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                let io = powershell(json!({"diskio":true}))
                    .await
                    .unwrap_or(Value::Null);
                (disks, io)
            }));
        }
        if self
            .storage_at
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(3600))
        {
            let binary = executable_bytes();
            let state = sqlite_state_bytes(&self.state_database);
            self.storage = Some(PlatformStorageMetric {
                collected_at: Utc::now(),
                binary_bytes: binary,
                state_bytes: state,
                web_bytes: None,
                database_bytes: None,
                total_bytes: binary.and_then(|a| state.and_then(|b| a.checked_add(b))),
            });
            self.storage_at = Some(Instant::now());
        }
        let fast = self.collect_fast();
        let networks = &self.cached_networks;
        let mut data = json!({"collected_at":Utc::now(),"cpu_usage_percent":fast.cpu_usage_percent,"load_average_one":null,"load_average_five":null,"load_average_fifteen":null,"memory_total_bytes":fast.memory_total_bytes,"memory_used_bytes":fast.memory_used_bytes,"swap_total_bytes":self.system.total_swap(),"swap_used_bytes":self.system.used_swap(),"uptime_seconds":System::uptime(),"disks":self.disks,"networks":networks,"gpus":self.gpus,"privilege":self.privilege,"platform_process":self.process.sample(),"platform_storage":self.storage,"collection_policy":self.policy,"fast_collected_at":fast.collected_at,"disk_collected_at":self.disk_at.map(|(_,at)|at),"gpu_collected_at":self.gpu_at.map(|(_,at)|at),"collection_issues":self.issues});
        for (key, value) in self.values.as_object().unwrap() {
            data[key] = value.clone();
        }
        let mut samples = InventorySamples::default();
        for (name, seconds, dest) in [
            ("users", 180, &mut samples.users),
            ("software", 3600, &mut samples.software),
            ("proxy", 300, &mut samples.proxy),
            ("programming", 43200, &mut samples.programming),
            ("ai_tools", 300, &mut samples.ai_tools),
        ] {
            if let Some((_, at)) = self.timestamps.get(name) {
                *dest = Some(InventorySample {
                    collected_at: *at,
                    interval_seconds: seconds,
                });
            }
        }
        data["inventory_samples"] = serde_json::to_value(samples).unwrap();
        data["inventory_collected_at"] = json!(self.timestamps.get("network").map(|(_, at)| at));
        serde_json::from_value(data)
            .expect("Windows collector constructs protocol-typed observations")
    }
}

/// Explicit read-only diagnostics. The caller supplies a fresh private test folder;
/// this path never reads production Agent configuration or contacts a controller.
pub(crate) async fn diagnostics(directory: &Path) -> Result<()> {
    anyhow::ensure!(!directory.exists(), "diagnostics requires a new directory");
    std::fs::create_dir(directory)?;
    let store = Store::open(&directory.join("diagnostic-state.db")).await?;
    let mut collector =
        NativeCollector::initialize(&directory.join("diagnostic-state.db"), &store).await?;
    let mut result = collector.collect().await;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        result = collector.collect().await;
        if collector.inventory_task.is_none() {
            break;
        }
    }
    println!(
        "{}",
        serde_json::to_string(&json!({"inventory":collector.inventory()?,"metrics":result}))?
    );
    Ok(())
}
