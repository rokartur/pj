//! usagefleet group budget of the active Claude account, cached and refreshed in the background.

use anyhow::Context;
use pj_ext::usagefleet::{Config, GroupView, LIMITS_URL};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(60);

struct Snapshot {
    label: Option<String>,
    fetched_at: Instant,
    view: Option<GroupView>,
}

#[derive(Default)]
struct State {
    snapshot: Option<Snapshot>,
    refreshing: bool,
    uuid_by_label: HashMap<String, String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

/// Cached view for the active Claude account. Starts a refresh when it is older than
/// `TTL` or belongs to the account this process switched away from.
pub(super) fn group_view() -> Option<GroupView> {
    // Unit tests must not read the real usagefleet config or reach the network.
    if cfg!(test) {
        return None;
    }
    let switched_to = crate::auth::claude::get_active_account_override();
    let mut state = STATE.lock().ok()?;
    let current = state
        .snapshot
        .as_ref()
        .filter(|snapshot| switched_to.is_none() || snapshot.label == switched_to);
    let view = current.and_then(|snapshot| snapshot.view.clone());
    let stale = current.is_none_or(|snapshot| snapshot.fetched_at.elapsed() >= TTL);
    if stale && !state.refreshing && tokio::runtime::Handle::try_current().is_ok() {
        state.refreshing = true;
        tokio::spawn(refresh());
    }
    view
}

async fn refresh() {
    let label = crate::auth::claude::active_account_label();
    let view = match label.as_deref() {
        Some(label) => fetch_view(label).await.unwrap_or_else(|error| {
            // Fails open like `usagefleet guard`: no reading means no block.
            crate::logging::warn(&format!("usagefleet: {error:#}"));
            None
        }),
        None => None,
    };
    let Ok(mut state) = STATE.lock() else {
        return;
    };
    let changed = state
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| snapshot.label != label || snapshot.view != view);
    state.snapshot = Some(Snapshot {
        label,
        fetched_at: Instant::now(),
        view,
    });
    state.refreshing = false;
    drop(state);
    if changed {
        crate::bus::Bus::global().publish(crate::bus::BusEvent::UsagefleetRefreshed);
    }
}

async fn fetch_view(label: &str) -> anyhow::Result<Option<GroupView>> {
    let config_path = dirs::home_dir()
        .context("no home directory")?
        .join(".config/usagefleet/config.json");
    let config: Config = match std::fs::read(&config_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("malformed usagefleet config")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading usagefleet config"),
    };
    let uuid = account_uuid(label).await?;
    let Some(token) = config.token_for(&uuid) else {
        return Ok(None);
    };
    let response = crate::provider::shared_http_client()
        .get(LIMITS_URL)
        .header("x-api-key", token)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    // A rejected token hides the group numbers rather than freezing old ones.
    if !response.status().is_success() {
        return Ok(None);
    }
    Ok(Some(response.json().await?))
}

async fn account_uuid(label: &str) -> anyhow::Result<String> {
    if let Some(uuid) = STATE
        .lock()
        .ok()
        .and_then(|state| state.uuid_by_label.get(label).cloned())
    {
        return Ok(uuid);
    }
    let credentials = crate::auth::claude::load_credentials_for_account(label)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let access_token =
        if credentials.expires_at < now_ms + 300_000 && !credentials.refresh_token.is_empty() {
            crate::auth::oauth::refresh_claude_tokens_for_account(&credentials.refresh_token, label)
                .await?
                .access_token
        } else {
            credentials.access_token
        };
    let uuid = crate::auth::oauth::fetch_claude_account_uuid(&access_token)
        .await?
        .context("Claude profile has no account uuid")?;
    if let Ok(mut state) = STATE.lock() {
        state.uuid_by_label.insert(label.to_string(), uuid.clone());
    }
    Ok(uuid)
}
