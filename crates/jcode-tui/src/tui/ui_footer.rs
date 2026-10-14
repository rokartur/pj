//! pj footer, ported from pi's status-footer: one row under the input, the
//! place on the left and the session on the right.
//!
//!   ~/Developer/pj master unstaged:2 +1 -1   limit 5h 14% 7d 21%  ctx 42k (21%)  opus 4.8 high

use super::TuiState;
use super::info_widget::{self, GitInfo, InfoWidgetData, UsageProvider};
use crate::tui::session_facts;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

/// A blank row keeps the footer off the input rail, then the footer row.
pub(super) const FOOTER_HEIGHT: u16 = 2;

const TEXT: Color = Color::Rgb(208, 208, 208);
const SOFT: Color = Color::Rgb(188, 188, 188);
const MUTED: Color = Color::Rgb(133, 133, 133);
const GREEN: Color = Color::Rgb(95, 215, 95);
const RED: Color = Color::Rgb(255, 95, 95);
const AMBER: Color = Color::Rgb(215, 175, 95);

pub(super) fn draw_footer(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    if area.height < FOOTER_HEIGHT || area.width == 0 {
        return;
    }
    let data = app.info_widget_data();
    let session = session_spans(app, &data);
    let context = context_spans(&data);
    // The floating overview already shows the limits.
    let limits = if info_widget::usage_limits_visible() {
        Vec::new()
    } else {
        limit_spans(&data)
    };
    // Narrow terminals drop the limits first, then the context; the model stays longest.
    let rights = [
        joined([limits, context.clone(), session.clone()]),
        joined([context, session.clone()]),
        session,
    ];
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

/// pi's git badges: branch, then only the counts that are non-zero.
fn git_spans(git: &GitInfo) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if !git.branch.trim().is_empty() {
        spans.push(Span::styled(
            format!(" {}", git.branch.trim()),
            Style::default().fg(MUTED),
        ));
    }
    let badges = [
        ("staged:", git.staged, GREEN),
        ("unstaged:", git.modified + git.untracked, AMBER),
        ("↑", git.ahead, MUTED),
        ("↓", git.behind, MUTED),
        ("+", git.added_total, GREEN),
        ("-", git.removed_total, RED),
    ];
    for (label, count, color) in badges {
        if count > 0 {
            spans.push(Span::styled(
                format!(" {label}{count}"),
                Style::default().fg(color),
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

/// Subscription windows as used percentages; API-key providers show their spend.
fn limit_spans(data: &InfoWidgetData) -> Vec<Span<'static>> {
    let Some(usage) = &data.usage_info else {
        return Vec::new();
    };
    let muted = Style::default().fg(MUTED);
    let text = Style::default().fg(TEXT);
    match usage.provider {
        UsageProvider::Anthropic | UsageProvider::OpenAI => {
            let mut spans = vec![Span::styled("limit", muted)];
            let windows = [
                (
                    usage.primary_limit_label.as_deref().unwrap_or("5h"),
                    usage.five_hour,
                ),
                (
                    usage.secondary_limit_label.as_deref().unwrap_or("7d"),
                    usage.seven_day,
                ),
            ];
            for (label, used) in windows {
                spans.push(Span::styled(format!(" {label}"), muted));
                spans.push(Span::styled(format!(" {:.0}%", used * 100.0), text));
            }
            spans
        }
        UsageProvider::CostBased if usage.total_cost > 0.0 => {
            vec![Span::styled(format!("${:.2}", usage.total_cost), text)]
        }
        _ => Vec::new(),
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
    fn git_badges_skip_zero_counts() {
        let git = GitInfo {
            branch: "main".into(),
            modified: 1,
            untracked: 1,
            ahead: 2,
            added_total: 5,
            ..Default::default()
        };
        let line = Line::from(git_spans(&git));
        assert_eq!(text(&line), " main unstaged:2 ↑2 +5");
    }

    #[test]
    fn context_and_limits_read_like_pi() {
        let data = InfoWidgetData {
            observed_context_tokens: Some(42_000),
            context_limit: Some(200_000),
            usage_info: Some(super::super::info_widget::UsageInfo {
                provider: UsageProvider::Anthropic,
                five_hour: 0.14,
                seven_day: 0.21,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(text(&Line::from(context_spans(&data))), "ctx 42k (21%)");
        assert_eq!(text(&Line::from(limit_spans(&data))), "limit 5h 14% 7d 21%");
    }
}
