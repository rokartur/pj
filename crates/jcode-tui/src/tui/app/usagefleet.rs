//! The active Claude account's display name and usagefleet group budget, cached and
//! refreshed in the background.

use anyhow::Context;
use pj_ext::usagefleet::{Config, GroupView, LIMITS_URL};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(60);

struct Snapshot {
    label: Option<String>,
    fetched_at: Instant,
    current: Option<ActiveClaude>,
}

#[derive(Clone, PartialEq)]
pub(super) struct ActiveClaude {
    /// The account label (claude-fox) or its email, per `display.account_name`.
    pub name: String,
    pub group: Option<GroupView>,
}

#[derive(Default)]
struct State {
    snapshot: Option<Snapshot>,
    refreshing: bool,
    uuid_by_label: HashMap<String, String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

/// Cached view of the active Claude account. Starts a refresh when it is older than
/// `TTL` or belongs to the account this process switched away from.
pub(super) fn active_claude() -> Option<ActiveClaude> {
    // Unit tests must not read the real auth files, usagefleet config or the network.
    if cfg!(test) {
        return None;
    }
    let switched_to = crate::auth::claude::get_active_account_override();
    let mut state = STATE.lock().ok()?;
    let snapshot = state
        .snapshot
        .as_ref()
        .filter(|snapshot| switched_to.is_none() || snapshot.label == switched_to);
    let current = snapshot.and_then(|snapshot| snapshot.current.clone());
    let stale = snapshot.is_none_or(|snapshot| snapshot.fetched_at.elapsed() >= TTL);
    if stale && !state.refreshing && tokio::runtime::Handle::try_current().is_ok() {
        state.refreshing = true;
        tokio::spawn(refresh());
    }
    current
}

async fn refresh() {
    let label = crate::auth::claude::active_account_label();
    let current = match label.as_deref() {
        Some(label) => Some(ActiveClaude {
            name: display_name(label),
            group: fetch_view(label).await.unwrap_or_else(|error| {
                // Fails open like `usagefleet guard`: no reading means no block.
                crate::logging::warn(&format!("usagefleet: {error:#}"));
                None
            }),
        }),
        None => None,
    };
    let Ok(mut state) = STATE.lock() else {
        return;
    };
    let changed = state
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| snapshot.label != label || snapshot.current != current);
    state.snapshot = Some(Snapshot {
        label,
        fetched_at: Instant::now(),
        current,
    });
    state.refreshing = false;
    drop(state);
    if changed {
        crate::bus::Bus::global().publish(crate::bus::BusEvent::UsagefleetRefreshed);
    }
}

/// An account saved before pj recorded emails has none; its label stands in.
fn display_name(label: &str) -> String {
    if !crate::config::config().display.account_name_email() {
        return label.to_string();
    }
    crate::auth::claude::list_accounts()
        .ok()
        .and_then(|accounts| accounts.into_iter().find(|account| account.label == label))
        .and_then(|account| account.email)
        .unwrap_or_else(|| label.to_string())
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
