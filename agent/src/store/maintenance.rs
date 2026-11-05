//! Infrequent, idle-only SQLite housekeeping; never discard queued reports.
use super::Store;
use anyhow::Result;
use sqlx::{Connection, SqliteConnection};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(3600);
const SQL_BUDGET: Duration = Duration::from_millis(250);
const MIN_RECLAIM: u64 = 16 * 1024 * 1024;
const MAX_LIVE_BYTES: u64 = 32 * 1024 * 1024;

pub(super) struct Maintenance {
    path: PathBuf,
    next: Mutex<Instant>,
}
impl Maintenance {
    pub(super) fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            next: Mutex::new(Instant::now() + Duration::from_secs(60)),
        }
    }
    fn claim(&self, now: Instant) -> bool {
        let Ok(mut next) = self.next.lock() else {
            return false;
        };
        if now < *next {
            return false;
        }
        *next = now + INTERVAL;
        true
    }
}

impl Store {
    pub(crate) async fn maintain_if_due(&self) {
        if self.maintenance.claim(Instant::now()) && self.maintain_once().await.is_err() {
            // No SQLite contents or credentials in diagnostics; a failure is
            // deferred to the next hour and never fails metrics reporting.
            tracing::warn!("Agent SQLite housekeeping deferred; queued data retained");
        }
    }

    pub(super) async fn maintain_once(&self) -> Result<bool> {
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM outbox)")
            .fetch_one(&self.pool)
            .await?;
        if pending {
            return Ok(false);
        }
        // A dedicated short-lived connection prevents an interrupted progress
        // handler or a short busy timeout leaking into the reporting pool.
        let options = (*self.pool.connect_options())
            .clone()
            .busy_timeout(Duration::from_millis(50));
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM outbox)")
            .fetch_one(&mut connection)
            .await?;
        if pending {
            return Ok(false);
        }
        let started = Instant::now();
        connection
            .lock_handle()
            .await?
            .set_progress_handler(1000, move || started.elapsed() < SQL_BUDGET);
        sqlx::query("DELETE FROM outbox_software WHERE digest IN (SELECT digest FROM outbox_software LIMIT 16) AND NOT EXISTS(SELECT 1 FROM outbox)")
            .execute(&mut connection)
            .await?;
        let pages: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&mut connection)
            .await?;
        let free: i64 = sqlx::query_scalar("PRAGMA freelist_count")
            .fetch_one(&mut connection)
            .await?;
        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&mut connection)
            .await?;
        let size = (pages.max(0) as u64).saturating_mul(page_size.max(0) as u64);
        let waste = (free.max(0) as u64).saturating_mul(page_size.max(0) as u64);
        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&mut connection)
            .await?;
        let reclaim = waste >= MIN_RECLAIM
            && waste >= size / 2
            && size.saturating_sub(waste) <= MAX_LIVE_BYTES;
        if reclaim {
            if mode == 0 {
                // Converting a legacy database requires one rebuild. SQLite
                // can need twice the original size as free working space.
                let required = size.saturating_mul(2).saturating_add(MIN_RECLAIM);
                if !available_bytes(&self.maintenance.path)
                    .await
                    .is_some_and(|free| free >= required)
                {
                    return Ok(false);
                }
                sqlx::query("PRAGMA auto_vacuum=INCREMENTAL")
                    .execute(&mut connection)
                    .await?;
            }
            let result = sqlx::query(if mode == 0 {
                "VACUUM"
            } else {
                "PRAGMA incremental_vacuum(4096)"
            })
            .execute(&mut connection)
            .await;
            connection.lock_handle().await?.remove_progress_handler();
            result?;
        }
        // Busy readers make this return without waiting; never force their
        // transactions closed or lower SQLite's existing synchronous setting.
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_all(&mut connection)
            .await?;
        connection.close().await?;
        Ok(reclaim)
    }
}

async fn available_bytes(path: &Path) -> Option<u64> {
    // Query only the state filesystem, never enumerate mounts or touch NFS
    // inventory. This child is started only for a large idle legacy database.
    let mut command = tokio::process::Command::new("/usr/bin/df");
    command
        .args(["-B1", "--output=avail", "--"])
        .arg(path)
        .env_clear()
        .env("LC_ALL", "C")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(1), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() || output.stdout.len() > 8192 {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .lines()
        .last()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintenance_waits_for_startup_and_stays_hourly() {
        let maintenance = Maintenance::new(Path::new("/unused-test-path"));
        let now = Instant::now();
        assert!(!maintenance.claim(now));
        assert!(maintenance.claim(now + Duration::from_secs(61)));
        assert!(!maintenance.claim(now + Duration::from_secs(62)));
        assert!(maintenance.claim(now + Duration::from_secs(3662)));
    }
}
