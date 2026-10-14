//! pj footer, ported from pi's status-footer: one row under the input, the
//! place on the left and the session on the right.
//!
//!   ~/Developer/pj master ↑2 ↓1   ctx 42k (21%)  opus 4.8 high

use super::TuiState;
use super::info_widget::{GitInfo, InfoWidgetData};
use crate::tui::session_facts;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

/// A blank row keeps the footer off the input rail, then the footer row.
pub(super) const FOOTER_HEIGHT: u16 = 2;

const TEXT: Color = Color::Rgb(208, 208, 208);
const SOFT: Color = Color::Rgb(188, 188, 188);
const MUTED: Color = Color::Rgb(133, 133, 133);

pub(super) fn draw_footer(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.height < FOOTER_HEIGHT || area.width == 0 {
        return;
    }
    let data = app.info_widget_data();
    let session = session_spans(app, &data);
    let context = context_spans(&data);
    // Limits live in the floating overview. Narrow terminals drop the context first.
    let rights = [joined([context, session.clone()]), session];
    let line = row(place_spans(app, &data), rights, area.width as usize);
    let row_area = Rect {
        y: area.y + 1,
        height: 1,
        ..area
    };
    frame.render_widget(Paragraph::new(line), row_area);
}

/// Left part, then the first right part that fits flush with the edge with a
/// two-column gap; with none fitting, only the left part.
fn row<const N: usize>(
    left: Vec<Span<'static>>,
    rights: [Vec<Span<'static>>; N],
    width: usize,
) -> Line<'static> {
    let left = Line::from(left);
    for right in rights {
        let right_width = Line::from(right.clone()).width();
        if right_width == 0 || left.width() + 2 + right_width > width {
            continue;
        }
        let gap = width - left.width() - right_width;
        let mut spans = left.spans;
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(right);
        return Line::from(spans);
    }
    super::truncate_line_with_ellipsis_to_width(&left, width)
}

/// The non-empty parts, two columns apart.
fn joined<const N: usize>(parts: [Vec<Span<'static>>; N]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for part in parts.into_iter().filter(|part| !part.is_empty()) {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.extend(part);
    }
    spans
}

fn place_spans(app: &dyn TuiState, data: &InfoWidgetData) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(dir) = app.working_dir() {
        spans.push(Span::styled(
            session_facts::dir_label(&dir),
            Style::default().fg(TEXT).bold(),
        ));
    }
    if let Some(git) = &data.git_info {
        spans.extend(git_spans(git));
    }
    spans
}

/// Branch, then commits to push and to pull when non-zero; the Changes widget
/// carries the working-tree counts.
fn git_spans(git: &GitInfo) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if !git.branch.trim().is_empty() {
        spans.push(Span::styled(
            format!(" {}", git.branch.trim()),
            Style::default().fg(MUTED),
        ));
    }
    for (arrow, count) in [("↑", git.ahead), ("↓", git.behind)] {
        if count > 0 {
            spans.push(Span::styled(
                format!(" {arrow}{count}"),
                Style::default().fg(MUTED),
            ));
        }
    }
    spans
}

fn session_spans(app: &dyn TuiState, data: &InfoWidgetData) -> Vec<Span<'static>> {
    let model = data
        .model
        .clone()
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| app.provider_model());
    if is_placeholder_model(&model) {
        return Vec::new();
    }
    let mut spans = vec![Span::styled(
        session_facts::pretty_model(&model).to_lowercase(),
        Style::default().fg(MUTED),
    )];
    if let Some(effort) = data.reasoning_effort.as_deref().and_then(short_reasoning) {
        spans.push(Span::styled(
            format!(" {effort}"),
            Style::default().fg(SOFT),
        ));
    }
    spans
}

/// Remote startup fills the model slot with connection progress, and a session
/// with no provider reports "unknown"; neither is a model name.
fn is_placeholder_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.is_empty()
        || model.starts_with("connecting")
        || model.starts_with("loading")
        || model == "connected"
        || model == "unknown"
}

fn short_reasoning(effort: &str) -> Option<&str> {
    match effort.trim() {
        "" => None,
        mode @ ("swarm" | "swarm-deep") => Some(crate::tui::app::effort_display_label(mode)),
        effort => Some(effort),
    }
}

fn context_spans(data: &InfoWidgetData) -> Vec<Span<'static>> {
    let used = match (data.observed_context_tokens, &data.context_info) {
        (Some(observed), _) => observed as usize,
        (None, Some(info)) if info.total_chars > 0 => info.estimated_tokens(),
        _ => return Vec::new(),
    };
    let limit = data
        .context_limit
        .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
        .max(1);
    let percent = (used * 100 + limit / 2) / limit;
    vec![
        Span::styled("ctx", Style::default().fg(MUTED)),
        Span::styled(
            format!(" {} ({percent}%)", format_tokens(used)),
            Style::default().fg(TEXT),
        ),
    ]
}

fn format_tokens(tokens: usize) -> String {
    match tokens {
        1_000_000.. => format!("{:.1}M", tokens as f64 / 1_000_000.0),
        1_000.. => format!("{}k", tokens / 1000),
        _ => tokens.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn row_takes_the_first_right_part_that_fits_and_drops_it_when_crowded() {
        let left = || vec![Span::raw("~/pj master")];
        let rights = || {
            [
                joined([vec![Span::raw("ctx 4k")], vec![Span::raw("opus")]]),
                vec![Span::raw("opus")],
            ]
        };
        assert_eq!(
            text(&row(left(), rights(), 26)),
            "~/pj master   ctx 4k  opus"
        );
        assert_eq!(text(&row(left(), rights(), 20)), "~/pj master     opus");
        assert_eq!(text(&row(left(), rights(), 16)), "~/pj master");
    }

    #[test]
    fn git_shows_only_branch_and_nonzero_ahead_behind() {
        let git = GitInfo {
            branch: "main".into(),
            modified: 1,
            untracked: 1,
            ahead: 2,
            added_total: 5,
            ..Default::default()
        };
        let line = Line::from(git_spans(&git));
        assert_eq!(text(&line), " main ↑2");
    }

    #[test]
    fn context_reads_like_pi() {
        let data = InfoWidgetData {
            observed_context_tokens: Some(42_000),
            context_limit: Some(200_000),
            ..Default::default()
        };
        assert_eq!(text(&Line::from(context_spans(&data))), "ctx 42k (21%)");
    }
}
