//! The terminal's native tab title, plus the transcript's cumulative edit-line
//! counts (the transcript's diff badges, not a repository-wide `git diff`).
use super::{App, DisplayMessage};

#[derive(Default)]
pub(super) struct TerminalTitleState {
    pub last_sent: String,
}

pub(super) fn edit_line_counts(message: &DisplayMessage) -> (usize, usize) {
    let Some(tool) = message.tool_data.as_ref().filter(|tool| {
        message.role == "tool"
            && crate::tui::ui::tools_ui::is_edit_tool_name(&tool.name)
            && jcode_tui_tool_display::concise_tool_error_summary(&message.content).is_none()
    }) else {
        return (0, 0);
    };
    crate::tui::ui_diff::diff_change_counts_for_tool(tool, &message.content)
}

impl App {
    /// Tell herdr (when running in a herdr pane) whether this client is
    /// working or idle and which session to resume. Tick-safe: unchanged
    /// state is a cheap comparison and reports are sent off-thread.
    pub(super) fn sync_herdr_agent_state(&self) {
        if self.suppress_terminal_title_updates || !self.is_remote_client() {
            return;
        }
        let state = if self.is_processing() {
            crate::tui::herdr::AgentState::Working
        } else {
            crate::tui::herdr::AgentState::Idle
        };
        crate::tui::herdr::sync(state, self.active_client_session_id());
    }

    pub(super) fn set_terminal_title(&self, title: String) {
        let mut state = self.terminal_title.borrow_mut();
        if state.last_sent != title
            && crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(&title)).is_ok()
        {
            state.last_sent = title;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolCall;

    #[test]
    fn terminal_title_counts_follow_append_remove_and_history_replace() {
        let mut app = super::super::tests::create_test_app();
        app.suppress_terminal_title_updates = true;
        let message = DisplayMessage::tool(
            "@@ -1 +1 @@\n-old\n+new\n+extra",
            ToolCall {
                id: "edit-1".into(),
                name: "edit".into(),
                input: serde_json::json!({"file_path": "example.rs"}),
                intent: None,
                thought_signature: None,
            },
        );
        app.push_display_message(message.clone());
        assert_eq!(app.display_edit_line_counts, (2, 1));
        app.push_display_message(DisplayMessage::user("continue"));
        assert_eq!(app.display_edit_line_counts, (2, 1));
        app.adjust_display_message_stats(&message, false);
        assert_eq!(app.display_edit_line_counts, (0, 0));
        app.replace_display_messages(vec![message.clone(), message]);
        assert_eq!(app.display_edit_line_counts, (4, 2));
        app.replace_display_messages(vec![]);
        assert_eq!(app.display_edit_line_counts, (0, 0));
    }

    #[test]
    fn edit_counts_ignore_failures_and_non_edit_output() {
        let mut message = DisplayMessage::tool(
            "@@ -1 +1 @@\n-old\n+new\n+extra",
            ToolCall {
                id: "edit-1".into(),
                name: "edit".into(),
                input: serde_json::json!({"file_path": "example.rs"}),
                intent: None,
                thought_signature: None,
            },
        );
        assert_eq!(edit_line_counts(&message), (2, 1));
        message.tool_data.as_mut().unwrap().name = "bash".into();
        assert_eq!(edit_line_counts(&message), (0, 0));
        message.tool_data.as_mut().unwrap().name = "edit".into();
        message.content = "Error: could not apply patch\n-old\n+new".into();
        assert_eq!(edit_line_counts(&message), (0, 0));
    }
}
