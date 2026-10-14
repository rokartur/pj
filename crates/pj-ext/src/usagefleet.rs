//! usagefleet group budget for the active Claude account, ported from pi's usagefleet extension.

use serde::Deserialize;
use std::collections::BTreeMap;

pub const LIMITS_URL: &str = "https://usagefleet.com/api/v1/limits";

/// `~/.config/usagefleet/config.json`: device tokens, one per Claude account uuid.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    token: Option<String>,
    #[serde(default, rename = "accountTokens")]
    account_tokens: BTreeMap<String, String>,
}

impl Config {
    /// Same precedence as the usagefleet CLI: the account's own token, the shared token, any token.
    pub fn token_for(&self, account_uuid: &str) -> Option<&str> {
        let non_empty = |token: &&String| !token.is_empty();
        self.account_tokens
            .get(account_uuid)
            .filter(non_empty)
            .or(self.token.as_ref().filter(non_empty))
            .or(self.account_tokens.values().find(non_empty))
            .map(String::as_str)
    }
}

/// Response of `GET /api/v1/limits`: the group budget the server enforces.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub group: Option<String>,
    pub session_pct: Option<f64>,
    pub weekly_pct: Option<f64>,
    #[serde(default)]
    pub blocked: bool,
    pub blocked_window: Option<String>,
    pub blocked_model: Option<String>,
    pub blocked_pct: Option<f64>,
    pub blocked_until: Option<String>,
}

/// The group's used percentages, rounded; over 100 when the group overran its budget.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupQuota {
    pub name: String,
    pub five_hour_used: u16,
    pub seven_day_used: u16,
}

/// None outside a group.
pub fn group_quota(view: &GroupView) -> Option<GroupQuota> {
    let used = |pct: f64| pct.round().max(0.0) as u16;
    Some(GroupQuota {
        name: view.group.clone()?,
        five_hour_used: used(view.session_pct?),
        seven_day_used: used(view.weekly_pct?),
    })
}

/// Mirrors `usagefleet guard`: why a prompt is refused, or None unless the server says `blocked`.
pub fn block_message(view: &GroupView, resets_in: Option<&str>) -> Option<String> {
    if !view.blocked {
        return None;
    }
    let window = if view.blocked_window.as_deref() == Some("weekly") {
        "weekly"
    } else {
        "5h"
    };
    let limit = match view.blocked_model.as_deref() {
        Some(model) if !model.is_empty() => format!("{model} {window}"),
        _ => window.to_string(),
    };
    let pct = view.blocked_pct.map_or("100".to_string(), |pct| {
        format!("{pct:.1}").trim_end_matches(".0").to_string()
    });
    let who = match view.group.as_deref() {
        Some(group) if !group.is_empty() => format!("\"{group}\""),
        _ => "this group".to_string(),
    };
    let resets = resets_in.map_or(String::new(), |resets| format!(" Resets in {resets}."));
    Some(format!(
        "usagefleet: {who} has used {pct}% of its {limit} budget, so new prompts are blocked.{resets}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(group: Option<&str>, session_pct: f64, weekly_pct: f64) -> GroupView {
        GroupView {
            group: group.map(str::to_string),
            session_pct: Some(session_pct),
            weekly_pct: Some(weekly_pct),
            blocked: false,
            blocked_window: None,
            blocked_model: None,
            blocked_pct: None,
            blocked_until: None,
        }
    }

    #[test]
    fn reads_the_group_quota_and_nothing_outside_a_group() {
        assert_eq!(
            group_quota(&view(Some("Artur"), 104.6, 17.2)),
            Some(GroupQuota {
                name: "Artur".to_string(),
                five_hour_used: 105,
                seven_day_used: 17,
            })
        );
        assert_eq!(group_quota(&view(None, 30.4, 17.0)), None);
    }

    #[test]
    fn resolves_the_token_of_the_account_then_the_shared_one() {
        let config: Config = serde_json::from_str(
            r#"{"token":"uf_shared","accountTokens":{"acc-1":"uf_acc1","acc-2":"uf_acc2"}}"#,
        )
        .unwrap();
        assert_eq!(config.token_for("acc-2"), Some("uf_acc2"));
        assert_eq!(config.token_for("acc-3"), Some("uf_shared"));

        let only_accounts: Config =
            serde_json::from_str(r#"{"accountTokens":{"acc-1":"uf_acc1"}}"#).unwrap();
        assert_eq!(only_accounts.token_for("acc-3"), Some("uf_acc1"));
        assert_eq!(Config::default().token_for("acc-1"), None);
    }

    #[test]
    fn refuses_a_prompt_only_on_an_explicit_block() {
        let blocked = GroupView {
            blocked: true,
            blocked_window: Some("weekly".to_string()),
            blocked_pct: Some(100.0),
            ..view(Some("Artur"), 100.0, 100.0)
        };
        assert_eq!(
            block_message(&blocked, None).as_deref(),
            Some(
                "usagefleet: \"Artur\" has used 100% of its weekly budget, so new prompts are blocked."
            )
        );
        let opus = GroupView {
            blocked_window: None,
            blocked_model: Some("opus".to_string()),
            blocked_pct: Some(87.25),
            ..blocked.clone()
        };
        assert_eq!(
            block_message(&opus, Some("3h 20m")).as_deref(),
            Some(
                "usagefleet: \"Artur\" has used 87.2% of its opus 5h budget, so new prompts are blocked. Resets in 3h 20m."
            )
        );
        assert_eq!(
            block_message(
                &GroupView {
                    blocked: false,
                    ..blocked
                },
                None
            ),
            None
        );
    }
}
