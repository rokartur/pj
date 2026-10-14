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

/// "Artur: 5h 30% · 7d 17%" (used, or left when `show_used` is false), or None outside a
/// group or when the group repeats the account's used %.
pub fn group_line(
    view: &GroupView,
    account_used_pct: Option<(f64, f64)>,
    show_used: bool,
) -> Option<String> {
    let group = view.group.as_deref()?;
    let used = (view.session_pct?.round(), view.weekly_pct?.round());
    // The server does not say how many groups there are; a sole group's budget is the whole account.
    if account_used_pct.map(|(five_hour, seven_day)| (five_hour.round(), seven_day.round()))
        == Some(used)
    {
        return None;
    }
    let shown = |pct: f64| {
        if show_used {
            pct
        } else {
            (100.0 - pct).max(0.0)
        }
    };
    Some(format!(
        "{group}: 5h {}% · 7d {}%",
        shown(used.0),
        shown(used.1)
    ))
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
    fn shows_the_group_budget_and_nothing_outside_a_group() {
        assert_eq!(
            group_line(&view(Some("Artur"), 30.4, 17.0), None, true).as_deref(),
            Some("Artur: 5h 30% · 7d 17%")
        );
        assert_eq!(
            group_line(&view(Some("Artur"), 104.4, 17.0), None, false).as_deref(),
            Some("Artur: 5h 0% · 7d 83%")
        );
        assert_eq!(group_line(&view(None, 30.4, 17.0), None, true), None);
    }

    #[test]
    fn hides_a_group_whose_budget_repeats_the_accounts() {
        let artur = view(Some("Artur"), 29.0, 52.0);
        assert_eq!(group_line(&artur, Some((29.2, 51.8)), true), None);
        assert_eq!(
            group_line(&artur, Some((58.0, 52.0)), true).as_deref(),
            Some("Artur: 5h 29% · 7d 52%")
        );
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
