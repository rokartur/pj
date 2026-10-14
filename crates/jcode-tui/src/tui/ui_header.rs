use super::box_utils::render_rounded_box;
use super::changelog::get_unseen_changelog_entries;
use super::{TuiState, dim_color, semver};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
#[cfg(test)]
use std::sync::OnceLock;

#[cfg(test)]
fn unseen_changelog_entries_override() -> &'static std::sync::Mutex<Option<Vec<String>>> {
    static OVERRIDE: OnceLock<std::sync::Mutex<Option<Vec<String>>>> = OnceLock::new();
    OVERRIDE.get_or_init(|| std::sync::Mutex::new(None))
}

fn unseen_changelog_entries() -> Vec<String> {
    #[cfg(test)]
    {
        if let Ok(guard) = unseen_changelog_entries_override().lock()
            && let Some(entries) = guard.clone()
        {
            return entries;
        }
    }
    get_unseen_changelog_entries().clone()
}

#[cfg(test)]
pub(crate) fn set_unseen_changelog_entries_override_for_tests(entries: Option<Vec<String>>) {
    let mut guard = unseen_changelog_entries_override()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = entries;
}

/// Keep an override in place only while its owning render test is in scope.
/// Callers must hold the shared render-state test lock before creating this guard.
#[cfg(test)]
pub(crate) struct ChangelogEntriesOverrideGuard;

#[cfg(test)]
pub(crate) fn scoped_unseen_changelog_entries_override_for_tests(
    entries: Vec<String>,
) -> ChangelogEntriesOverrideGuard {
    set_unseen_changelog_entries_override_for_tests(Some(entries));
    ChangelogEntriesOverrideGuard
}

#[cfg(test)]
impl Drop for ChangelogEntriesOverrideGuard {
    fn drop(&mut self) {
        set_unseen_changelog_entries_override_for_tests(None);
    }
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

fn semver_core() -> String {
    semver()
        .split('-')
        .next()
        .unwrap_or_else(semver)
        .to_string()
}

/// Build the "Updates" rounded box (unseen release notes) so it can be
/// rendered inside the top padding above the header. `max_lines` bounds the
/// total height including the box borders; entries beyond the budget are
/// collapsed into a "…N more" line. Returns an empty vec when there are no
/// unseen entries or the budget/width is too small for a box.
pub(super) fn build_updates_box_lines(width: u16, max_lines: usize) -> Vec<Line<'static>> {
    let w = width as usize;
    if w <= 20 || max_lines < 3 {
        return Vec::new();
    }
    let new_entries = unseen_changelog_entries();
    if new_entries.is_empty() {
        return Vec::new();
    }

    // Budget for content lines inside the box (borders take 2 lines).
    let content_budget = (max_lines - 2).min(8);
    let has_more = new_entries.len() > content_budget;
    let display_count = if has_more {
        content_budget.saturating_sub(1)
    } else {
        new_entries.len()
    };

    let mut content: Vec<Line> = Vec::new();
    for entry in new_entries.iter().take(display_count) {
        content.push(Line::from(Span::styled(
            format!("• {}", entry),
            Style::default().fg(dim_color()),
        )));
    }
    if has_more {
        content.push(Line::from(Span::styled(
            format!(
                "  …{} more · /changelog to see all",
                new_entries.len() - display_count
            ),
            Style::default().fg(dim_color()),
        )));
    }
    if content.is_empty() {
        return Vec::new();
    }

    render_rounded_box(
        "Updates",
        content,
        w.saturating_sub(2),
        Style::default().fg(dim_color()),
    )
    .into_iter()
    .map(|line| line.alignment(Alignment::Left))
    .collect()
}

/// pj's header, pi style: name and version, then the skill count. Provider,
/// auth and update details live in `/model`, `/login` and `/usage`.
pub(in crate::tui) fn build_header_sections(
    app: &dyn TuiState,
    _width: u16,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let faint = Style::default().fg(rgb(98, 98, 98));
    let mut lines = vec![Line::from(vec![
        Span::styled("pj", Style::default().fg(rgb(238, 238, 238)).bold()),
        Span::styled(format!(" {}", semver_core()), faint),
    ])];
    let skills = app.available_skills().len();
    if skills > 0 {
        lines.push(Line::from(Span::styled(format!("skills {skills}"), faint)));
    }
    (lines, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::OnceLock;

    #[test]
    fn changelog_override_is_cleared_after_a_render_test_panics() {
        let _lock = crate::tui::ui::render_state_test_lock();
        let panic = std::panic::catch_unwind(|| {
            let _fixture = scoped_unseen_changelog_entries_override_for_tests(vec![
                "temporary changelog entry".to_owned(),
            ]);
            assert_eq!(unseen_changelog_entries(), ["temporary changelog entry"]);
            panic!("injected render failure");
        });
        assert!(panic.is_err());
        assert!(
            unseen_changelog_entries_override()
                .lock()
                .unwrap()
                .is_none(),
            "a failed render test must not leak its changelog fixture"
        );
    }

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            Err(anyhow::anyhow!(
                "Mock provider should not be used for streaming completions in ui header tests"
            ))
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockProvider)
        }
    }

    fn ensure_test_jcode_home_if_unset() {
        static TEST_HOME: OnceLock<std::path::PathBuf> = OnceLock::new();

        if std::env::var_os("JCODE_HOME").is_some() {
            return;
        }

        let path = TEST_HOME.get_or_init(|| {
            let path = std::env::temp_dir().join(format!("jcode-test-home-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&path);
            path
        });
        crate::env::set_var("JCODE_HOME", path);
    }

    fn create_test_app() -> crate::tui::app::App {
        ensure_test_jcode_home_if_unset();

        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let rt = tokio::runtime::Runtime::new().expect("test runtime");
        let registry = rt.block_on(Registry::new(provider.clone()));
        crate::tui::app::App::new_for_test_harness(provider, registry)
    }

    #[test]
    fn header_is_name_version_and_skill_count() {
        let app = create_test_app();
        let (persistent, secondary) = build_header_sections(&app, 80);
        let text: Vec<String> = persistent
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(text[0], format!("pj v{}", jcode_build_meta::semver()));
        assert!(text.len() <= 2, "{text:?}");
        assert!(secondary.is_empty());
    }
}
