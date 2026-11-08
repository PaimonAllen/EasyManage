//! Persisted master and local-source switches; cancellations never create failures.
use super::*;

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub(super) struct Settings {
    pub enabled: bool,
    pub local_enabled: bool,
    pub revision: i64,
    pub global_epoch: i64,
    pub local_epoch: i64,
}
impl Settings {
    pub fn allows(self, local_source: bool) -> bool {
        self.enabled && (!local_source || self.local_enabled && local::enabled())
    }
    fn same_epoch(self, other: Self, local_source: bool) -> bool {
        self.global_epoch == other.global_epoch
            && (!local_source || self.local_epoch == other.local_epoch)
    }
}
fn row(row: &sqlx::postgres::PgRow) -> Settings {
    Settings {
        enabled: row.get("enabled"),
        local_enabled: row.get("local_enabled"),
        revision: row.get("revision"),
        global_epoch: row.get("global_epoch"),
        local_epoch: row.get("local_epoch"),
    }
}
fn apply(state: &Lab, value: Settings) {
    if state.shared.monitoring.send_if_modified(|current| {
        if value.revision <= current.revision {
            return false;
        }
        *current = value;
        true
    }) {
        state.shared.notify.notify_one();
    }
}
pub(super) async fn read(state: &Lab) -> Result<Settings, sqlx::Error> {
    let value=row(&sqlx::query("SELECT enabled,local_enabled,revision,global_epoch,local_epoch FROM proxy_lab_monitoring WHERE singleton").fetch_one(&state.shared.pool).await?);
    apply(state, value);
    Ok(value)
}
pub(super) async fn interrupted(
    receiver: &mut watch::Receiver<Settings>,
    initial: Settings,
    local_source: bool,
) {
    loop {
        let current = *receiver.borrow_and_update();
        if !current.allows(local_source) || !current.same_epoch(initial, local_source) {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Toggle {
    enabled: bool,
    expected_revision: i64,
    #[serde(default)]
    confirmed: bool,
}
async fn change(
    state: Lab,
    headers: HeaderMap,
    uri: Uri,
    input: Toggle,
    local_source: bool,
) -> ApiResult<Value> {
    origin(&headers, &uri)?;
    if !local_source && !input.confirmed {
        return Err(error(StatusCode::BAD_REQUEST, "操作所有检测前必须二次确认"));
    }
    let _lock = state.shared.mutations.lock().await;
    let mut tx = state.shared.pool.begin().await.map_err(db)?;
    let old=row(&sqlx::query("SELECT enabled,local_enabled,revision,global_epoch,local_epoch FROM proxy_lab_monitoring WHERE singleton FOR UPDATE").fetch_one(&mut *tx).await.map_err(db)?);
    if old.revision != input.expected_revision {
        return Err(error(
            StatusCode::CONFLICT,
            "检测开关已变化，请刷新状态后重试",
        ));
    }
    let value = if (if local_source {
        old.local_enabled
    } else {
        old.enabled
    }) != input.enabled
    {
        let sql = if local_source {
            "UPDATE proxy_lab_monitoring SET local_enabled=$1,revision=revision+1,local_epoch=local_epoch+1,updated_at=NOW() WHERE singleton RETURNING enabled,local_enabled,revision,global_epoch,local_epoch"
        } else {
            "UPDATE proxy_lab_monitoring SET enabled=$1,revision=revision+1,global_epoch=global_epoch+1,updated_at=NOW() WHERE singleton RETURNING enabled,local_enabled,revision,global_epoch,local_epoch"
        };
        row(&sqlx::query(sql)
            .bind(input.enabled)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?)
    } else {
        old
    };
    tx.commit().await.map_err(db)?;
    apply(&state, value);
    let message = if local_source {
        if input.enabled {
            "本机网站检测已启用；总开关关闭时仍保持暂停"
        } else {
            "本机网站检测已暂停，历史保留"
        }
    } else if input.enabled {
        "所有检测总开关已打开，各配置恢复原开关选择；IP 评分仍需手动发起"
    } else {
        "所有网络检测已暂停，正在取消运行中的批次；配置选择和历史均保留"
    };
    Ok(Json(json!({"monitoring":value,"message":message})))
}
pub(super) async fn global(
    State(state): State<Lab>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Toggle>,
) -> ApiResult<Value> {
    change(state, headers, uri, input, false).await
}
pub(super) async fn local_source(
    State(state): State<Lab>,
    headers: HeaderMap,
    uri: Uri,
    Json(input): Json<Toggle>,
) -> ApiResult<Value> {
    change(state, headers, uri, input, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn enabled() -> Settings {
        Settings {
            enabled: true,
            local_enabled: true,
            revision: 1,
            global_epoch: 1,
            local_epoch: 1,
        }
    }
    #[tokio::test]
    async fn pause_resume_epochs_cancel_old_work_even_when_changes_are_coalesced() {
        let initial = enabled();
        let (sender, mut receiver) = watch::channel(initial);
        sender.send_modify(|v| {
            v.enabled = false;
            v.global_epoch += 1;
        });
        sender.send_modify(|v| {
            v.enabled = true;
            v.global_epoch += 1;
        });
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            interrupted(&mut receiver, initial, false),
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn local_switch_does_not_cancel_proxy_checks_and_master_preserves_choices() {
        let initial = enabled();
        let (sender, mut receiver) = watch::channel(initial);
        sender.send_modify(|v| {
            v.local_enabled = false;
            v.local_epoch += 1;
        });
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                interrupted(&mut receiver, initial, false)
            )
            .await
            .is_err()
        );
        assert!(receiver.borrow().allows(false));
        assert!(!receiver.borrow().allows(true));
        sender.send_modify(|v| v.enabled = false);
        assert!(!receiver.borrow().allows(false));
        sender.send_modify(|v| v.enabled = true);
        assert!(!receiver.borrow().local_enabled);
    }
}
