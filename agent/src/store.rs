use anyhow::{Context, Result};
use easymanage_protocol::{AgentReport, HardwareDetails};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use uuid::Uuid;

const OUTBOX_LIMIT: i64 = 1_000;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn proxy_enabled(&self) -> Result<bool> {
        match self.metadata("proxy_enabled").await?.as_deref() {
            None | Some("true") => Ok(true),
            Some("false") => Ok(false),
            _ => anyhow::bail!("stored proxy collection setting is invalid"),
        }
    }

    pub async fn save_proxy_enabled(&self, enabled: bool) -> Result<()> {
        self.set_metadata("proxy_enabled", if enabled { "true" } else { "false" })
            .await
    }

    pub async fn agent_control(&self) -> Result<easymanage_protocol::AgentControl> {
        match self.metadata("agent_control").await? {
            Some(value) => {
                serde_json::from_str(&value).context("stored monitoring control is invalid")
            }
            None => Ok(Default::default()),
        }
    }
    pub async fn save_agent_control(
        &self,
        control: &easymanage_protocol::AgentControl,
    ) -> Result<()> {
        self.set_metadata("agent_control", &serde_json::to_string(control)?)
            .await
    }
    pub async fn permissions_enabled(&self) -> Result<bool> {
        match self.metadata("permissions_enabled").await?.as_deref() {
            None | Some("true") => Ok(true),
            Some("false") => Ok(false),
            _ => anyhow::bail!("stored permission collection setting is invalid"),
        }
    }

    pub async fn save_permissions_enabled(&self, enabled: bool) -> Result<()> {
        self.set_metadata(
            "permissions_enabled",
            if enabled { "true" } else { "false" },
        )
        .await
    }

    pub async fn users_enabled(&self) -> Result<bool> {
        match self.metadata("users_enabled").await?.as_deref() {
            None | Some("true") => Ok(true),
            Some("false") => Ok(false),
            _ => anyhow::bail!("stored user collection setting is invalid"),
        }
    }

    pub async fn save_users_enabled(&self, enabled: bool) -> Result<()> {
        self.set_metadata("users_enabled", if enabled { "true" } else { "false" })
            .await
    }

    pub async fn hardware_details(&self, uid: u32) -> Result<Option<HardwareDetails>> {
        let Some(value) = self.metadata("hardware_details_v1").await? else {
            return Ok(None);
        };
        // A privilege change invalidates Basic/Full observations immediately.
        let Ok((cached_uid, details)) = serde_json::from_str::<(u32, HardwareDetails)>(&value)
        else {
            return Ok(None);
        };
        Ok((cached_uid == uid && details.valid()).then_some(details))
    }

    pub async fn save_hardware_details(&self, uid: u32, details: &HardwareDetails) -> Result<()> {
        self.set_metadata(
            "hardware_details_v1",
            &serde_json::to_string(&(uid, details))?,
        )
        .await
    }

    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(parent).with_context(|| {
                format!("failed to create state directory {}", parent.display())
            })?;
        }
        let mut file_options = std::fs::OpenOptions::new();
        file_options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file_options.mode(0o600);
        }
        let state_file = file_options
            .open(path)
            .context("failed to open agent state file")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            state_file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .context("failed to open agent state database")?;

        sqlx::raw_sql(
            r"
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            ",
        )
        .execute(&pool)
        .await
        .context("failed to initialize agent state database")?;

        Ok(Self { pool })
    }

    pub async fn node_id(&self) -> Result<Uuid> {
        let requested = std::env::var("EASYMANAGE_NODE_ID")
            .ok()
            .map(|value| Uuid::parse_str(&value).context("EASYMANAGE_NODE_ID is invalid"))
            .transpose()?;
        if let Some(value) = self.metadata("node_id").await? {
            let stored = Uuid::parse_str(&value).context("stored node_id is invalid")?;
            if requested.is_some_and(|requested| requested != stored) {
                anyhow::bail!("EASYMANAGE_NODE_ID differs from the persisted node identity");
            }
            return Ok(stored);
        }

        let id = requested.unwrap_or_else(Uuid::now_v7);
        self.set_metadata("node_id", &id.to_string()).await?;
        Ok(id)
    }

    pub async fn agent_token(&self) -> Result<Option<String>> {
        self.metadata("agent_token").await
    }

    pub async fn set_agent_token(&self, token: &str) -> Result<()> {
        self.set_metadata("agent_token", token).await
    }

    pub async fn enqueue(&self, report: &AgentReport) -> Result<i64> {
        let payload =
            serde_json::to_string(report).context("failed to serialize metrics report")?;
        let mut transaction = self.pool.begin().await?;
        let id = sqlx::query("INSERT INTO outbox (payload, created_at) VALUES (?1, ?2)")
            .bind(payload)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&mut *transaction)
            .await?
            .last_insert_rowid();
        sqlx::query(
            "DELETE FROM outbox WHERE id NOT IN (SELECT id FROM outbox ORDER BY id DESC LIMIT ?1)",
        )
        .bind(OUTBOX_LIMIT)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(id)
    }

    pub async fn oldest(&self) -> Result<Option<(i64, AgentReport)>> {
        let row = sqlx::query("SELECT id, payload FROM outbox ORDER BY id LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            let id = row.try_get::<i64, _>("id")?;
            let payload = row.try_get::<String, _>("payload")?;
            let report =
                serde_json::from_str(&payload).context("stored metrics report is invalid")?;
            Ok((id, report))
        })
        .transpose()
    }

    pub async fn acknowledge(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM outbox WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn metadata(&self, key: &str) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT value FROM metadata WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(Into::into)
    }

    async fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO metadata (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{OUTBOX_LIMIT, Store};
    use easymanage_protocol::AgentReport;
    use serde_json::json;
    use std::path::PathBuf;
    use uuid::Uuid;

    struct TemporaryState {
        directory: PathBuf,
    }

    impl TemporaryState {
        fn new() -> Self {
            // VM/bwrap test runner provides the temporary filesystem. Never reuse a real agent path.
            let directory =
                std::env::temp_dir().join(format!("easymanage-state-test-{}", Uuid::new_v4()));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory).expect("private test directory");
            Self { directory }
        }

        fn database(&self) -> PathBuf {
            self.directory.join("path with spaces/state.db")
        }
    }

    impl Drop for TemporaryState {
        fn drop(&mut self) {
            // This unique directory is owned exclusively by this test and contains no host state.
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn report(node_id: Uuid, uptime: u64) -> AgentReport {
        serde_json::from_value(json!({
            "protocol_version": easymanage_protocol::PROTOCOL_VERSION,
            "node_id": node_id,
            "agent_version": "test",
            "inventory": {
                "hostname": "isolated-test-node", "os_name": "Linux", "os_version": "test",
                "kernel_version": "test", "architecture": "test", "cpu_model": "test",
                "cpu_logical_cores": 1, "cpu_physical_cores": 1
            },
            "metrics": {
                "collected_at": "2026-09-21T00:00:00Z", "cpu_usage_percent": 0.0,
                "load_average_one": 0.0, "load_average_five": 0.0, "load_average_fifteen": 0.0,
                "memory_total_bytes": 1024, "memory_used_bytes": 128,
                "swap_total_bytes": 0, "swap_used_bytes": 0, "uptime_seconds": uptime,
                "disks": [], "networks": [], "gpus": []
            }
        }))
        .expect("valid test report")
    }

    #[tokio::test]
    async fn user_collection_switch_survives_reopen() {
        let state = TemporaryState::new();
        let store = Store::open(&state.database()).await.unwrap();
        assert!(store.users_enabled().await.unwrap());
        assert!(store.permissions_enabled().await.unwrap());
        assert!(store.proxy_enabled().await.unwrap());
        assert_eq!(
            store.agent_control().await.unwrap(),
            easymanage_protocol::AgentControl::default()
        );
        let paused = easymanage_protocol::AgentControl {
            revision: 7,
            paused: true,
        };
        store.save_agent_control(&paused).await.unwrap();
        store.save_users_enabled(false).await.unwrap();
        store.save_permissions_enabled(false).await.unwrap();
        store.save_proxy_enabled(false).await.unwrap();
        store.pool.close().await;
        let reopened = Store::open(&state.database()).await.unwrap();
        assert!(!reopened.users_enabled().await.unwrap());
        assert!(!reopened.permissions_enabled().await.unwrap());
        assert!(!reopened.proxy_enabled().await.unwrap());
        assert_eq!(reopened.agent_control().await.unwrap(), paused);
        reopened.save_users_enabled(true).await.unwrap();
        assert!(reopened.users_enabled().await.unwrap());
        assert!(
            !reopened.permissions_enabled().await.unwrap(),
            "user switch does not change permission collection"
        );
        reopened.save_permissions_enabled(true).await.unwrap();
        reopened.save_proxy_enabled(true).await.unwrap();
        assert!(reopened.proxy_enabled().await.unwrap());
        assert!(reopened.permissions_enabled().await.unwrap());
    }

    #[tokio::test]
    async fn hardware_cache_is_scoped_to_privilege() {
        let temporary = TemporaryState::new();
        let store = Store::open(&temporary.database()).await.unwrap();
        let details: easymanage_protocol::HardwareDetails = serde_json::from_value(json!({
            "collected_at": "2026-10-07T00:00:00Z", "processors": ["fixture"],
            "motherboard": [], "memory": [], "graphics": [], "displays": [],
            "disks": [], "audio": [], "network": [], "notes": []
        }))
        .unwrap();
        store.save_hardware_details(1001, &details).await.unwrap();
        assert!(store.hardware_details(0).await.unwrap().is_none());
        let cached = store.hardware_details(1001).await.unwrap().unwrap();
        assert_eq!(cached.collected_at, details.collected_at);
        assert_eq!(cached.processors, details.processors);
    }

    #[tokio::test]
    async fn node_identity_and_credentials_survive_reopen() {
        let state = TemporaryState::new();
        let store = Store::open(&state.database())
            .await
            .expect("open new state");
        // No process-environment mutations: these tests can safely execute concurrently.
        let original = store.node_id().await.expect("initial node identity");
        store
            .set_agent_token("test-persisted-agent-credential")
            .await
            .expect("persist token");
        assert_eq!(store.node_id().await.expect("stable identity"), original);
        store.pool.close().await;

        let reopened = Store::open(&state.database())
            .await
            .expect("reopen existing state");
        assert_eq!(reopened.node_id().await.expect("same identity"), original);
        assert_eq!(
            reopened
                .agent_token()
                .await
                .expect("persisted credential")
                .as_deref(),
            Some("test-persisted-agent-credential")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(state.database())
                    .expect("state permissions")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        reopened.pool.close().await;
    }

    #[tokio::test]
    async fn unacknowledged_reports_survive_failure_and_process_restart() {
        let state = TemporaryState::new();
        let store = Store::open(&state.database()).await.expect("open queue");
        let node_id = store.node_id().await.expect("node identity");
        store
            .enqueue(&report(node_id, 10))
            .await
            .expect("first sample");
        store
            .enqueue(&report(node_id, 20))
            .await
            .expect("second sample");
        let (first_id, first) = store
            .oldest()
            .await
            .expect("read first")
            .expect("queued sample");
        assert_eq!(first.metrics.uptime_seconds, 10);
        // A timeout/rejected report never calls acknowledge; repeat reads must preserve it.
        assert_eq!(
            store
                .oldest()
                .await
                .expect("retry")
                .expect("still queued")
                .0,
            first_id
        );
        store.pool.close().await;

        let reopened = Store::open(&state.database()).await.expect("reopen queue");
        let (retry_id, retry) = reopened
            .oldest()
            .await
            .expect("retry after restart")
            .expect("durable sample");
        assert_eq!(retry_id, first_id);
        assert_eq!(retry.node_id, node_id);
        reopened
            .acknowledge(retry_id)
            .await
            .expect("acknowledge accepted sample");
        let (second_id, second) = reopened
            .oldest()
            .await
            .expect("next sample")
            .expect("second remains");
        assert_eq!(second.metrics.uptime_seconds, 20);
        assert!(second_id > retry_id);
        // Repeated ACKs are harmless and cannot delete another sample.
        reopened.acknowledge(retry_id).await.expect("duplicate ACK");
        assert_eq!(
            reopened
                .oldest()
                .await
                .expect("read after duplicate ACK")
                .expect("second remains")
                .0,
            second_id
        );
        reopened.acknowledge(second_id).await.expect("final ACK");
        assert!(reopened.oldest().await.expect("empty queue").is_none());
        reopened.pool.close().await;
    }

    #[tokio::test]
    async fn latest_acknowledgement_preserves_unaccepted_history() {
        let state = TemporaryState::new();
        let store = Store::open(&state.database()).await.expect("open queue");
        let node_id = store.node_id().await.expect("node identity");
        let first_id = store
            .enqueue(&report(node_id, 10))
            .await
            .expect("historical sample");
        let second_id = store
            .enqueue(&report(node_id, 20))
            .await
            .expect("historical sample");
        let latest_id = store
            .enqueue(&report(node_id, 30))
            .await
            .expect("fresh heartbeat");
        assert!(first_id < second_id && second_id < latest_id);
        // The current heartbeat is sent first even when history is pending; its ACK is precise.
        store.acknowledge(latest_id).await.expect("latest accepted");
        let (queued_id, queued) = store
            .oldest()
            .await
            .expect("oldest pending")
            .expect("history remains");
        assert_eq!(queued_id, first_id);
        assert_eq!(queued.metrics.uptime_seconds, 10);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox")
            .fetch_one(&store.pool)
            .await
            .expect("pending history count");
        assert_eq!(count, 2);
        let acknowledged_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE id = ?1")
                .bind(latest_id)
                .fetch_one(&store.pool)
                .await
                .expect("latest removed");
        assert_eq!(acknowledged_count, 0);
        store.pool.close().await;
    }

    #[tokio::test]
    async fn long_offline_queue_keeps_newest_samples_with_bounded_growth() {
        let state = TemporaryState::new();
        let store = Store::open(&state.database()).await.expect("open queue");
        let node_id = store.node_id().await.expect("node identity");
        let payload = serde_json::to_string(&report(node_id, 1)).expect("fixture payload");
        // Seed an offline backlog in one transaction instead of a thousand slow per-sample writes.
        sqlx::query(
            "WITH RECURSIVE sequence(number) AS (SELECT 1 UNION ALL SELECT number + 1 FROM sequence WHERE number < ?1) \
             INSERT INTO outbox(payload, created_at) SELECT ?2, '2026-09-21T00:00:00Z' FROM sequence",
        )
        .bind(OUTBOX_LIMIT)
        .bind(payload)
        .execute(&store.pool)
        .await
        .expect("offline backlog");
        let first_id = store
            .oldest()
            .await
            .expect("first queued")
            .expect("backlog")
            .0;
        store
            .enqueue(&report(node_id, 999))
            .await
            .expect("fresh sample");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox")
            .fetch_one(&store.pool)
            .await
            .expect("bounded queue count");
        assert_eq!(count, OUTBOX_LIMIT);
        assert!(
            store
                .oldest()
                .await
                .expect("remaining backlog")
                .expect("backlog retained")
                .0
                > first_id
        );
        let latest: String =
            sqlx::query_scalar("SELECT payload FROM outbox ORDER BY id DESC LIMIT 1")
                .fetch_one(&store.pool)
                .await
                .expect("latest sample retained");
        assert_eq!(
            serde_json::from_str::<AgentReport>(&latest)
                .expect("latest report")
                .metrics
                .uptime_seconds,
            999
        );
        store.pool.close().await;
    }
}
