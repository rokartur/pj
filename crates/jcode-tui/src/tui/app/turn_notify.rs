//! Desktop notification for completed agent turns.
//!
//! When a turn finishes (optionally only after a configurable duration), the
//! user gets a compact desktop notification: session name + duration in the
//! title, todo progress and a short snippet of the final assistant text in the
//! body. By default it fires only while the terminal window is unfocused.

use super::App;
use crate::todo::TodoItem;
use base64::Engine as _;
use std::io::Write;

/// Maximum characters of assistant text shown in the notification body.
/// Notification banners truncate aggressively; keep the payload tight.
const SNIPPET_MAX_CHARS: usize = 120;

/// Per-todo title clip length for the notification body. Banners are narrow and
/// we may show two todos, so keep each tight.
const TODO_MAX_CHARS: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TurnNotification {
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
}

impl App {
    /// Send a desktop notification for a just-completed turn when warranted.
    /// Call at turn completion, after the final assistant message is committed.
    pub(super) fn maybe_notify_turn_complete(&self, duration_secs: Option<f32>) {
        if !self.turn_notifications_wanted() {
            return;
        }
        let Some(duration) = duration_secs else {
            return;
        };

        let cfg = &crate::config::config().notifications;
        let todos = self
            .active_client_session_id()
            .map(load_session_todos)
            .unwrap_or_default();
        let threshold = if todos.is_empty() {
            cfg.turn_complete_min_secs
        } else {
            cfg.turn_complete_todo_min_secs
                .min(cfg.turn_complete_min_secs)
        };
        if duration < threshold as f32 {
            return;
        }

        let notification = build_turn_notification(
            self.active_client_session_id()
                .and_then(crate::id::extract_session_name),
            duration,
            &todos,
            self.last_assistant_text_for_notification().as_deref(),
        );
        self.send_turn_notification(&notification);
    }

    /// Notify that the agent stopped and waits on the user: a question to answer
    /// or an error no automatic retry will recover. No duration threshold.
    pub(super) fn notify_needs_attention(&self, reason: &str, detail: &str) {
        if !self.turn_notifications_wanted() {
            return;
        }
        let notification = build_attention_notification(
            self.active_client_session_id()
                .and_then(crate::id::extract_session_name),
            reason,
            detail,
        );
        self.send_turn_notification(&notification);
    }

    fn turn_notifications_wanted(&self) -> bool {
        let cfg = &crate::config::config().notifications;
        matches!(self.runtime_mode(), super::AppRuntimeMode::RemoteClient)
            && !self.is_replay
            && cfg.turn_complete
            && !(cfg.turn_complete_only_when_unfocused && self.client_focused())
    }

    fn send_turn_notification(&self, notification: &TurnNotification) {
        let sound = crate::config::config()
            .notifications
            .turn_complete_sound
            .trim();
        let sound = (!sound.is_empty()).then_some(sound);
        if !send_originating_terminal_notification(
            notification,
            self.active_client_session_id().unwrap_or("unknown"),
            sound,
        ) {
            crate::notifications::send_desktop_notification_rich(
                &notification.title,
                notification.subtitle.as_deref(),
                &notification.body,
                sound,
            );
        }
    }

    /// Final assistant text of the turn, used for the notification snippet.
    fn last_assistant_text_for_notification(&self) -> Option<String> {
        self.display_messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant" && !m.content.trim().is_empty())
            .map(|m| m.content.clone())
    }
}

/// Notification protocol of the terminal that hosts this client. Detected
/// from env vars that survive SSH (`TERM`, `LC_TERMINAL`) and tmux (vars
/// inherited from the terminal that started the tmux server).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalProtocol {
    /// OSC 99
    Kitty,
    /// OSC 777;notify: Ghostty, WezTerm, foot
    Osc777,
    /// OSC 9
    Iterm,
}

fn detect_terminal_protocol(env: impl Fn(&str) -> Option<String>) -> Option<TerminalProtocol> {
    let term_program = env("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
    let term = env("TERM").unwrap_or_default().to_ascii_lowercase();
    if env("KITTY_WINDOW_ID").is_some() || term_program == "kitty" || term == "xterm-kitty" {
        return Some(TerminalProtocol::Kitty);
    }
    if env("GHOSTTY_RESOURCES_DIR").is_some()
        || env("WEZTERM_PANE").is_some()
        || matches!(term_program.as_str(), "ghostty" | "wezterm")
        || matches!(term.as_str(), "xterm-ghostty" | "wezterm")
        || term.starts_with("foot")
    {
        return Some(TerminalProtocol::Osc777);
    }
    if term_program == "iterm.app"
        || env("LC_TERMINAL").is_some_and(|value| value == "iTerm2")
        || env("ITERM_SESSION_ID").is_some()
    {
        return Some(TerminalProtocol::Iterm);
    }
    None
}

fn in_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|value| !value.trim().is_empty())
}

/// Ask the terminal to create the notification when it has a native protocol.
///
/// Kitty and OSC 777 terminals post the notification themselves, so a click
/// focuses the exact originating pane, and it works over SSH. On macOS other
/// terminals use the bundled broker, which records the tty to return Terminal.app
/// and iTerm2 users to the originating tab.
fn send_originating_terminal_notification(
    notification: &TurnNotification,
    session_id: &str,
    sound: Option<&str>,
) -> bool {
    let protocol = detect_terminal_protocol(|key| std::env::var(key).ok());
    let tmux = in_tmux();
    // tmux drops passthrough unless `allow-passthrough on`, so inside tmux the
    // macOS broker is the reliable route.
    let terminal_focuses_origin = matches!(
        protocol,
        Some(TerminalProtocol::Kitty | TerminalProtocol::Osc777)
    ) && !tmux;
    if !terminal_focuses_origin
        && crate::notifications::send_macos_turn_notification(
            &notification.title,
            notification.subtitle.as_deref(),
            &notification.body,
            sound,
        )
    {
        return true;
    }

    let Some(protocol) = protocol else {
        return false;
    };
    let sequence = match protocol {
        TerminalProtocol::Kitty => kitty_notification_sequence(notification, session_id),
        TerminalProtocol::Osc777 => osc777_notification_sequence(notification),
        TerminalProtocol::Iterm => iterm_notification_sequence(notification),
    };
    let sequence = if tmux {
        tmux_passthrough(&sequence)
    } else {
        sequence
    };

    // This runs on the TUI event thread, after the completed turn has rendered,
    // so one atomic write and flush cannot interleave with a frame draw.
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(sequence.as_bytes()).is_ok() && stdout.flush().is_ok()
}

fn tmux_passthrough(sequence: &str) -> String {
    format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
}

fn notification_text(notification: &TurnNotification) -> String {
    match notification.subtitle.as_deref() {
        Some(subtitle) => format!("{}\n{}", subtitle, notification.body),
        None => notification.body.clone(),
    }
}

/// OSC 9 and 777 payloads are single-line, so the subtitle joins the body.
fn single_line_text(notification: &TurnNotification) -> String {
    match notification.subtitle.as_deref() {
        Some(subtitle) => format!("{} · {}", subtitle, notification.body),
        None => notification.body.clone(),
    }
}

fn osc_safe(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn kitty_notification_id(session_id: &str) -> String {
    let safe: String = session_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+' | '.'))
        .take(128)
        .collect();
    format!(
        "jcode-turn-{}",
        if safe.is_empty() { "unknown" } else { &safe }
    )
}

fn kitty_notification_sequence(notification: &TurnNotification, session_id: &str) -> String {
    // OSC 99 is Kitty's desktop-notification protocol. Notifications are tied
    // to the originating Kitty window, which is what makes click-to-focus work.
    // Base64 is required because the body can contain a newline and OSC 99's
    // unencoded form forbids every C0/C1 control character.
    let encoder = base64::engine::general_purpose::STANDARD;
    let title = encoder.encode(notification.title.as_bytes());
    let body = encoder.encode(notification_text(notification).as_bytes());
    let id = kitty_notification_id(session_id);
    // Request focus explicitly instead of relying on Kitty's current default.
    // This is the protocol guarantee that clicking the completed notification
    // returns to the exact window that emitted it.
    format!(
        "\x1b]99;i={id}:d=0:e=1:p=title;{title}\x1b\\\x1b]99;i={id}:d=1:e=1:p=body:a=focus;{body}\x1b\\"
    )
}

fn osc777_notification_sequence(notification: &TurnNotification) -> String {
    // The title ends at the first ';', the body takes the rest.
    let title = osc_safe(&notification.title).replace(';', ",");
    let body = osc_safe(&single_line_text(notification));
    format!("\x1b]777;notify;{title};{body}\x07")
}

fn iterm_notification_sequence(notification: &TurnNotification) -> String {
    // iTerm2's OSC 9 notification is likewise associated with its source tab.
    let text = osc_safe(&format!(
        "{}: {}",
        notification.title,
        single_line_text(notification)
    ));
    format!("\x1b]9;{text}\x07")
}

fn load_session_todos(session_id: &str) -> Vec<TodoItem> {
    crate::todo::load_todos(session_id).unwrap_or_default()
}

/// Build the compact notification. Kept free of `App` for testability.
///
/// Layout (macOS):
///   title:    jcode · <session> · done in <dur>
///   subtitle: <todo progress, e.g. "3/5 todos · 1 blocked">
///   body:     names the work — "✓ <just done> · → <in progress>" or a
///             blocker ("⊘ <todo> needs <dep>"), falling back to the
///             assistant snippet when there are no todos.
pub(super) fn build_turn_notification(
    session_name: Option<&str>,
    duration_secs: f32,
    todos: &[TodoItem],
    last_assistant_text: Option<&str>,
) -> TurnNotification {
    let mut title = String::from("jcode");
    if let Some(name) = session_name {
        title.push_str(" · ");
        title.push_str(name);
    }
    title.push_str(" · done in ");
    title.push_str(&format_duration_compact(duration_secs));

    let subtitle = todo_progress_line(todos);

    // Prefer naming the actual work; fall back to the assistant snippet.
    let work_line = todo_work_line(todos);
    let snippet = last_assistant_text
        .map(summary_snippet)
        .filter(|s| !s.is_empty());

    let mut body = String::new();
    if let Some(work) = work_line {
        body.push_str(&work);
    } else if let Some(snippet) = snippet {
        body.push_str(&snippet);
    }
    if body.is_empty() {
        body.push_str("Turn finished");
    }

    TurnNotification {
        title,
        subtitle,
        body,
    }
}

/// title: `jcode · <session> · <reason>`, body: first line of `detail`.
pub(super) fn build_attention_notification(
    session_name: Option<&str>,
    reason: &str,
    detail: &str,
) -> TurnNotification {
    let mut title = String::from("jcode");
    if let Some(name) = session_name {
        title.push_str(" · ");
        title.push_str(name);
    }
    title.push_str(" · ");
    title.push_str(reason);
    TurnNotification {
        title,
        subtitle: None,
        body: summary_snippet(detail),
    }
}

/// "3/5 todos" plus "· 1 blocked" when relevant; None when no todos exist.
fn todo_progress_line(todos: &[TodoItem]) -> Option<String> {
    if todos.is_empty() {
        return None;
    }
    let total = todos.len();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let blocked = todos
        .iter()
        .filter(|t| t.status != "completed" && !t.blocked_by.is_empty())
        .count();
    let mut line = if completed == total {
        format!("✓ all {} todos", total)
    } else {
        format!("{}/{} todos", completed, total)
    };
    if blocked > 0 {
        line.push_str(&format!(" · {} blocked", blocked));
    }
    Some(line)
}

/// Names the salient todo work for the body: a blocker if one is the reason the
/// turn stopped, otherwise the most recently completed item and what's next.
/// Returns None when there are no todos (caller falls back to the snippet).
fn todo_work_line(todos: &[TodoItem]) -> Option<String> {
    if todos.is_empty() {
        return None;
    }

    // A blocked, not-yet-done todo is the most actionable thing to surface.
    if let Some(blocked) = todos
        .iter()
        .find(|t| t.status != "completed" && !t.blocked_by.is_empty())
    {
        let dep = blocked
            .blocked_by
            .iter()
            .find_map(|id| resolve_todo_title(todos, id))
            .unwrap_or_else(|| blocked.blocked_by.join(", "));
        return Some(format!(
            "⊘ {} needs {}",
            clip_todo(&blocked.content),
            clip_todo(&dep)
        ));
    }

    let in_progress = todos
        .iter()
        .find(|t| t.status == "in_progress" || t.status == "in-progress");
    let last_done = todos.iter().rev().find(|t| t.status == "completed");

    let mut parts = Vec::new();
    if let Some(done) = last_done {
        let mut seg = format!("✓ {}", clip_todo(&done.content));
        if let Some(conf) = done.completion_confidence
            && conf == crate::todo::ConfidenceState::Speculative
        {
            seg.push_str(&format!(" (low conf: {})", conf.as_str()));
        }
        parts.push(seg);
    }
    if let Some(next) = in_progress {
        parts.push(format!("→ {}", clip_todo(&next.content)));
    } else if last_done.is_none() {
        // Nothing completed and nothing in progress: name the next pending item.
        if let Some(pending) = todos.iter().find(|t| t.status == "pending") {
            parts.push(format!("→ {}", clip_todo(&pending.content)));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn resolve_todo_title(todos: &[TodoItem], id: &str) -> Option<String> {
    todos.iter().find(|t| t.id == id).map(|t| t.content.clone())
}

/// Clip a single todo title for inline display in the notification body.
fn clip_todo(s: &str) -> String {
    let cleaned = strip_markdown_inline(s.trim());
    truncate_chars(cleaned.trim(), TODO_MAX_CHARS)
}

/// First meaningful line of the assistant text, markdown-stripped and clipped.
fn summary_snippet(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty() && !l.starts_with("```") && !l.starts_with('|') && !l.starts_with("---")
        })
        .unwrap_or("");
    let cleaned = strip_markdown_inline(line);
    truncate_chars(cleaned.trim(), SNIPPET_MAX_CHARS)
}

fn strip_markdown_inline(line: &str) -> String {
    let line = line.trim_start_matches('#').trim_start();
    // List/quote markers.
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("> "))
        .unwrap_or(line);
    line.replace("**", "").replace('`', "")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn format_duration_compact(secs: f32) -> String {
    let secs = secs.max(0.0) as u64;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{}m", m)
        } else {
            format!("{}m {}s", m, s)
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{}h", h)
        } else {
            format!("{}h {}m", h, m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(status: &str, blocked: bool) -> TodoItem {
        todo_named("x", status, &[]).tap(|t| {
            if blocked {
                t.blocked_by = vec!["other".to_string()];
            }
        })
    }

    fn todo_named(content: &str, status: &str, blocked_by: &[&str]) -> TodoItem {
        TodoItem {
            content: content.to_string(),
            status: status.to_string(),
            priority: "medium".to_string(),
            id: content.to_string(),
            group: None,
            confidence: None,
            completion_confidence: None,
            confidence_history: Vec::new(),
            blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            assigned_to: None,
        }
    }

    trait Tap: Sized {
        fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
    impl Tap for TodoItem {}

    #[test]
    fn title_includes_session_and_compact_duration() {
        let n = build_turn_notification(Some("fox"), 754.0, &[], Some("All done."));
        assert_eq!(n.title, "jcode · fox · done in 12m 34s");
        assert_eq!(n.subtitle, None);
        assert_eq!(n.body, "All done.");
    }

    #[test]
    fn subtitle_holds_progress_and_body_names_the_work() {
        let todos = vec![
            todo_named("wire up parser", "completed", &[]),
            todo_named("handle reconnect", "in_progress", &[]),
        ];
        let n = build_turn_notification(None, 200.0, &todos, Some("Fixed the parser bug."));
        assert_eq!(n.title, "jcode · done in 3m 20s");
        assert_eq!(n.subtitle.as_deref(), Some("1/2 todos"));
        // Names actual todo work, not the prose snippet.
        assert_eq!(n.body, "✓ wire up parser · → handle reconnect");
    }

    #[test]
    fn body_names_blocker_and_its_dependency() {
        let todos = vec![
            todo_named("run migration", "pending", &[]),
            todo_named("deploy", "pending", &["run migration"]),
        ];
        let n = build_turn_notification(None, 200.0, &todos, None);
        assert_eq!(n.subtitle.as_deref(), Some("0/2 todos · 1 blocked"));
        assert_eq!(n.body, "⊘ deploy needs run migration");
    }

    #[test]
    fn low_confidence_completion_is_flagged() {
        let mut done = todo_named("risky refactor", "completed", &[]);
        done.completion_confidence = Some(crate::todo::ConfidenceState::from_legacy_score(35));
        let n = build_turn_notification(None, 200.0, &[done], None);
        assert_eq!(n.subtitle.as_deref(), Some("✓ all 1 todos"));
        assert_eq!(n.body, "✓ risky refactor (low conf: speculative)");
    }

    #[test]
    fn all_complete_celebrated_in_subtitle() {
        let done = vec![
            todo_named("a", "completed", &[]),
            todo_named("b", "completed", &[]),
        ];
        let n = build_turn_notification(None, 200.0, &done, None);
        assert_eq!(n.subtitle.as_deref(), Some("✓ all 2 todos"));
        assert_eq!(n.body, "✓ b");
    }

    #[test]
    fn snippet_used_when_no_todos() {
        let n = build_turn_notification(None, 200.0, &[], Some("Fixed the parser bug."));
        assert_eq!(n.subtitle, None);
        assert_eq!(n.body, "Fixed the parser bug.");
    }

    #[test]
    fn snippet_skips_markdown_noise_and_truncates() {
        let text = "```rust\ncode\n```\n\n## **Results** are `good`\nmore detail";
        assert_eq!(summary_snippet(text), "code");

        let text = "\n\n- **Fixed** the `frobnicator`\nrest";
        assert_eq!(summary_snippet(text), "Fixed the frobnicator");

        let long = "a".repeat(300);
        let s = summary_snippet(&long);
        assert_eq!(s.chars().count(), SNIPPET_MAX_CHARS);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn empty_inputs_fall_back_to_minimal_body() {
        let n = build_turn_notification(None, 65.0, &[], None);
        assert_eq!(n.title, "jcode · done in 1m 5s");
        assert_eq!(n.subtitle, None);
        assert_eq!(n.body, "Turn finished");
    }

    #[test]
    fn still_counts_blocked_in_subtitle() {
        let blocked = vec![todo("completed", false), todo("pending", true)];
        let n = build_turn_notification(None, 200.0, &blocked, None);
        assert_eq!(n.subtitle.as_deref(), Some("1/2 todos · 1 blocked"));
    }

    #[test]
    fn attention_notification_names_reason_and_clips_detail() {
        let n = build_attention_notification(
            Some("fox"),
            "question",
            "## Which **DB**?\nPostgres or SQLite",
        );
        assert_eq!(n.title, "jcode · fox · question");
        assert_eq!(n.subtitle, None);
        assert_eq!(n.body, "Which DB?");
    }

    #[test]
    fn duration_formats_hours() {
        assert_eq!(format_duration_compact(59.0), "59s");
        assert_eq!(format_duration_compact(60.0), "1m");
        assert_eq!(format_duration_compact(3600.0), "1h");
        assert_eq!(format_duration_compact(3725.0), "1h 2m");
    }

    #[test]
    fn kitty_notification_is_one_completed_clickable_message() {
        let n = TurnNotification {
            title: "jcode · fox".to_string(),
            subtitle: Some("2/3 todos".to_string()),
            body: "Finished parser".to_string(),
        };
        assert_eq!(
            kitty_notification_sequence(&n, "session:fox/123"),
            "\x1b]99;i=jcode-turn-sessionfox123:d=0:e=1:p=title;amNvZGUgwrcgZm94\x1b\\\x1b]99;i=jcode-turn-sessionfox123:d=1:e=1:p=body:a=focus;Mi8zIHRvZG9zCkZpbmlzaGVkIHBhcnNlcg==\x1b\\"
        );
    }

    #[test]
    fn terminal_notification_payload_strips_osc_terminators() {
        let n = TurnNotification {
            title: "unsafe\x1b] title".to_string(),
            subtitle: None,
            body: "body\x07text".to_string(),
        };
        let sequence = iterm_notification_sequence(&n);
        assert_eq!(sequence.matches('\x07').count(), 1);
        assert!(!sequence.contains("\x1b] title"));
    }

    #[test]
    fn osc777_notification_keeps_title_and_body_apart() {
        let n = TurnNotification {
            title: "jcode · a;b".to_string(),
            subtitle: Some("1/2 todos".to_string()),
            body: "x;y\nz".to_string(),
        };
        assert_eq!(
            osc777_notification_sequence(&n),
            "\x1b]777;notify;jcode · a,b;1/2 todos · x;y z\x07"
        );
    }

    #[test]
    fn terminal_protocol_detected_over_ssh_and_tmux() {
        let detect = |vars: &[(&str, &str)]| {
            detect_terminal_protocol(|key| {
                vars.iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.to_string())
            })
        };
        assert_eq!(
            detect(&[("TERM", "xterm-ghostty")]),
            Some(TerminalProtocol::Osc777)
        );
        assert_eq!(
            detect(&[("TERM", "tmux-256color"), ("GHOSTTY_RESOURCES_DIR", "/x")]),
            Some(TerminalProtocol::Osc777)
        );
        assert_eq!(detect(&[("TERM", "foot")]), Some(TerminalProtocol::Osc777));
        assert_eq!(
            detect(&[("WEZTERM_PANE", "0")]),
            Some(TerminalProtocol::Osc777)
        );
        assert_eq!(
            detect(&[("TERM", "xterm-kitty")]),
            Some(TerminalProtocol::Kitty)
        );
        assert_eq!(
            detect(&[("LC_TERMINAL", "iTerm2")]),
            Some(TerminalProtocol::Iterm)
        );
        assert_eq!(detect(&[("TERM", "xterm-256color")]), None);
    }

    #[test]
    fn tmux_passthrough_doubles_escapes() {
        assert_eq!(
            tmux_passthrough("\x1b]99;x\x1b\\"),
            "\x1bPtmux;\x1b\x1b]99;x\x1b\x1b\\\x1b\\"
        );
    }
}
