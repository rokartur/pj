use super::frame::{self, Framed};
use super::{InfoWidgetData, UsageInfo, UsageProvider};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

/// Border layout: `Claude limits` top-left with one bar per window in the
/// body; cost-based providers put the dollar total on the top border.
pub(super) fn render_usage_widget(data: &InfoWidgetData, inner: Rect) -> Framed {
    let Some(info) = &data.usage_info else {
        return Framed::default();
    };
    if !info.available {
        return if matches!(info.provider, UsageProvider::Anthropic) {
            Framed::body(vec![usage_unavailable_line()]).title(frame::label("Anthropic limits"))
        } else {
            Framed::default()
        };
    }

    match info.provider {
        UsageProvider::Copilot => Framed::body(vec![Line::from(vec![Span::styled(
            format!(
                "{} in + {} out",
                format_tokens(info.input_tokens),
                format_tokens(info.output_tokens)
            ),
            Style::default().fg(rgb(140, 140, 150)),
        )])])
        .title(frame::label("Copilot tokens")),
        UsageProvider::CostBased => Framed::body(vec![Line::from(vec![Span::styled(
            format!(
                "{} in + {} out",
                format_tokens(info.input_tokens),
                format_tokens(info.output_tokens)
            ),
            Style::default().fg(rgb(140, 140, 150)),
        )])])
        .title(Line::from(vec![
            Span::styled("💰 ", Style::default().fg(rgb(140, 180, 255))),
            Span::styled(
                format!("${:.4}", info.total_cost),
                Style::default().fg(rgb(180, 180, 190)).bold(),
            ),
        ])),
        _ => {
            let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
            let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
            let five_hr_left = 100u8.saturating_sub(five_hr_used);
            let seven_day_left = 100u8.saturating_sub(seven_day_used);

            let five_hr_reset = info
                .five_hour_resets_at
                .as_deref()
                .map(crate::usage::format_reset_time);
            let seven_day_reset = info
                .seven_day_resets_at
                .as_deref()
                .map(crate::usage::format_reset_time);

            let mut lines = Vec::new();
            let label = info.provider.label();
            let title = if label.is_empty() {
                frame::label("Limits")
            } else {
                frame::label(format!("{} limits", label))
            };
            if let Some(primary_label) = info.primary_limit_label.as_deref() {
                lines.push(render_labeled_bar(
                    primary_label,
                    five_hr_used,
                    five_hr_left,
                    five_hr_reset.as_deref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            if let Some(secondary_label) = info.secondary_limit_label.as_deref() {
                lines.push(render_labeled_bar(
                    secondary_label,
                    seven_day_used,
                    seven_day_left,
                    seven_day_reset.as_deref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            if let Some(spark_usage) = info.spark {
                let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
                let spark_left = 100u8.saturating_sub(spark_used);
                let spark_reset = info
                    .spark_resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time);
                lines.push(render_labeled_bar(
                    "Spark",
                    spark_used,
                    spark_left,
                    spark_reset.as_deref(),
                    inner.width,
                    data.usage_display_used,
                ));
            }
            Framed::body(lines).title(title)
        }
    }
}

fn usage_unavailable_line() -> Line<'static> {
    Line::from(Span::styled(
        "Usage unavailable (refresh pending or failed)",
        Style::default().fg(rgb(255, 190, 80)),
    ))
}

pub(super) fn render_usage_compact(
    info: &UsageInfo,
    usage_display_used: bool,
) -> Vec<Line<'static>> {
    if !info.available {
        return if matches!(info.provider, UsageProvider::Anthropic) {
            vec![usage_unavailable_line()]
        } else {
            Vec::new()
        };
    }

    if matches!(info.provider, UsageProvider::CostBased) {
        return vec![Line::from(vec![Span::styled(
            format!(
                "${:.4} · {} in + {} out",
                info.total_cost,
                format_tokens(info.input_tokens),
                format_tokens(info.output_tokens)
            ),
            Style::default().fg(rgb(140, 140, 150)),
        )])];
    }

    let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
    let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
    let five_hr_left = 100u8.saturating_sub(five_hr_used);
    let seven_day_left = 100u8.saturating_sub(seven_day_used);
    let five_hr_reset = info
        .five_hour_resets_at
        .as_deref()
        .map(crate::usage::format_reset_time);
    let seven_day_reset = info
        .seven_day_resets_at
        .as_deref()
        .map(crate::usage::format_reset_time);

    let mut lines = Vec::new();
    if let Some(primary_label) = info.primary_limit_label.as_deref() {
        lines.push(render_compact_bar(
            primary_label,
            five_hr_used,
            five_hr_left,
            five_hr_reset.as_deref(),
            usage_display_used,
        ));
    }
    if let Some(secondary_label) = info.secondary_limit_label.as_deref() {
        lines.push(render_compact_bar(
            secondary_label,
            seven_day_used,
            seven_day_left,
            seven_day_reset.as_deref(),
            usage_display_used,
        ));
    }
    if let Some(spark_usage) = info.spark {
        let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
        let spark_left = 100u8.saturating_sub(spark_used);
        let spark_reset = info
            .spark_resets_at
            .as_deref()
            .map(crate::usage::format_reset_time);
        lines.push(render_compact_bar(
            "Spark",
            spark_used,
            spark_left,
            spark_reset.as_deref(),
            usage_display_used,
        ));
    }
    lines
}

/// Overview quota row: `5h ▰▰▰▱▱▱▱▱▱▱ 77% · 3h 40m`. The percentage follows
/// `usage_display_used`; the bar always fills with usage.
fn render_compact_bar(
    label: &str,
    used_pct: u8,
    left_pct: u8,
    reset_time: Option<&str>,
    usage_display_used: bool,
) -> Line<'static> {
    const BAR_WIDTH: usize = 10;
    let color = remaining_color(left_pct);
    let filled = ((used_pct as f32 / 100.0) * BAR_WIDTH as f32).round() as usize;
    let display_pct = if usage_display_used {
        used_pct
    } else {
        left_pct
    };
    let suffix = match reset_time {
        Some(reset) if left_pct == 0 => format!(" resets {reset}"),
        Some(reset) => format!(" {display_pct}% · {reset}"),
        None => format!(" {display_pct}%"),
    };
    Line::from(vec![
        Span::styled(
            format!("{} ", short_window_label(label)),
            Style::default().fg(rgb(140, 140, 150)),
        ),
        Span::styled("▰".repeat(filled), Style::default().fg(color)),
        Span::styled(
            "▱".repeat(BAR_WIDTH.saturating_sub(filled)),
            Style::default().fg(rgb(50, 50, 60)),
        ),
        Span::styled(suffix, Style::default().fg(color)),
    ])
}

fn short_window_label(label: &str) -> &str {
    match label {
        "5-hour" => "5h",
        "Weekly" => "7d",
        other => other,
    }
}

fn remaining_color(left_pct: u8) -> Color {
    if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    }
}

fn render_labeled_bar(
    label: &str,
    used_pct: u8,
    left_pct: u8,
    reset_time: Option<&str>,
    width: u16,
    usage_display_used: bool,
) -> Line<'static> {
    let color = remaining_color(left_pct);

    const LABEL_WIDTH: usize = 7;
    const MIN_BAR_WIDTH: usize = 4;

    let (display_pct, display_word) = if usage_display_used {
        (used_pct, "used")
    } else {
        (left_pct, "left")
    };
    let percentage_suffix = format!(" {}% {}", display_pct, display_word);
    let full_suffix = match reset_time {
        Some(reset) if left_pct == 0 => format!(" resets {}", reset),
        Some(reset) => format!("{} · {}", percentage_suffix, reset),
        None => percentage_suffix.clone(),
    };
    // On narrow widgets keep the percentage wording unambiguous, dropping the
    // reset countdown before sacrificing the bar. Exhausted wording is unchanged.
    let suffix = match reset_time {
        Some(_) if left_pct > 0 => {
            let budget = usize::from(width).saturating_sub(LABEL_WIDTH + MIN_BAR_WIDTH);
            if UnicodeWidthStr::width(full_suffix.as_str()) <= budget {
                full_suffix
            } else {
                percentage_suffix
            }
        }
        _ => full_suffix,
    };
    let suffix_width = UnicodeWidthStr::width(suffix.as_str());
    let label_width = LABEL_WIDTH.min(usize::from(width).saturating_sub(suffix_width));
    let bar_width = usize::from(width)
        .saturating_sub(label_width + suffix_width)
        .min(12);

    let filled = ((used_pct as f32 / 100.0) * bar_width as f32).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    let bar_filled = "▰".repeat(filled);
    let bar_empty = "▱".repeat(empty);

    let visible_label: String = label.chars().take(label_width).collect();
    let padded_label = format!("{visible_label:<label_width$}");

    Line::from(vec![
        Span::styled(padded_label, Style::default().fg(rgb(140, 140, 150))),
        Span::styled(bar_filled, Style::default().fg(color)),
        Span::styled(bar_empty, Style::default().fg(rgb(50, 50, 60))),
        Span::styled(suffix, Style::default().fg(color)),
    ])
}

/// pj Overview, one row per budget, always in used %:
/// `claude-fox 5h 48% · 7d 69% · 2h 30m` then `Artur      5h 49% · 7d 69%`.
pub(super) fn render_claude_quota(
    account: &str,
    usage: Option<&UsageInfo>,
    group: Option<&pj_ext::usagefleet::GroupQuota>,
) -> Vec<Line<'static>> {
    let name_width = group
        .map_or(0, |group| group.name.width())
        .max(account.width());
    let mut lines = Vec::new();
    if let Some(usage) = usage.filter(|usage| usage.available) {
        let percent = |fraction: f32| (fraction * 100.0).round().max(0.0) as u16;
        let reset = usage
            .five_hour_resets_at
            .as_deref()
            .map(crate::usage::format_reset_time);
        lines.push(quota_row(
            account,
            name_width,
            percent(usage.five_hour),
            percent(usage.seven_day),
            reset,
        ));
    }
    if let Some(group) = group {
        lines.push(quota_row(
            &group.name,
            name_width,
            group.five_hour_used,
            group.seven_day_used,
            None,
        ));
    }
    lines
}

fn quota_row(
    name: &str,
    name_width: usize,
    five_hour_used: u16,
    seven_day_used: u16,
    reset: Option<String>,
) -> Line<'static> {
    let dim = Style::default().fg(rgb(140, 140, 150));
    let used = |pct: u16| {
        let left = 100u16.saturating_sub(pct) as u8;
        Span::styled(
            format!("{pct}%"),
            Style::default().fg(remaining_color(left)),
        )
    };
    let padding = " ".repeat(name_width.saturating_sub(name.width()));
    let mut spans = vec![
        Span::raw(format!("{name}{padding}")),
        Span::styled(" 5h ", dim),
        used(five_hour_used),
        Span::styled(" · 7d ", dim),
        used(seven_day_used),
    ];
    if let Some(reset) = reset {
        spans.push(Span::styled(format!(" · {reset}"), dim));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn usage_bar_shows_reset_countdown_before_exhaustion() {
        let text = line_text(&render_labeled_bar(
            "5-hour",
            38,
            62,
            Some("4h 5m"),
            40,
            false,
        ));

        assert!(text.contains("62% left · 4h 5m"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 40);
    }

    #[test]
    fn usage_bar_keeps_wording_unambiguous_within_narrow_width() {
        let text = line_text(&render_labeled_bar(
            "Weekly",
            19,
            81,
            Some("1d 4h"),
            23,
            true,
        ));

        assert!(text.contains("19% used"));
        assert!(!text.contains("1d 4h"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 23);
        assert!(text.contains('▰') || text.contains('▱'));
    }

    #[test]
    fn used_wording_does_not_change_remaining_budget_color_thresholds() {
        let left = render_labeled_bar("5-hour", 85, 15, None, 24, false);
        let used = render_labeled_bar("5-hour", 85, 15, None, 24, true);

        assert!(line_text(&left).contains("15% left"));
        assert!(line_text(&used).contains("85% used"));
        assert_eq!(left.spans[1].style.fg, Some(rgb(255, 100, 100)));
        assert_eq!(used.spans[1].style.fg, left.spans[1].style.fg);
    }

    #[test]
    fn exhausted_usage_bar_preserves_resets_wording_and_width() {
        let text = line_text(&render_labeled_bar("5-hour", 100, 0, Some("12m"), 24, true));

        assert!(text.contains("resets 12m"));
        assert!(!text.contains("0% left"));
        assert!(!text.contains("100% used"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 24);
    }

    #[test]
    fn unavailable_anthropic_usage_is_explicit_not_silently_hidden() {
        let info = UsageInfo {
            provider: UsageProvider::Anthropic,
            available: false,
            ..Default::default()
        };

        let text = render_usage_compact(&info, false)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Usage unavailable"));
    }

    #[test]
    fn openai_monthly_usage_renders_only_the_reported_window() {
        let info = UsageInfo {
            provider: UsageProvider::OpenAI,
            primary_limit_label: Some("Monthly".to_string()),
            five_hour: 1.0,
            available: true,
            ..Default::default()
        };

        let lines = render_usage_compact(&info, false);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("Monthly"));
        assert!(!text.contains("5h"));
        assert!(!text.contains("7d"));
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn overview_bars_use_short_labels_without_left_word() {
        let info = UsageInfo {
            provider: UsageProvider::Anthropic,
            primary_limit_label: Some("5-hour".to_string()),
            five_hour: 0.23,
            five_hour_resets_at: None,
            secondary_limit_label: Some("Weekly".to_string()),
            seven_day: 0.66,
            available: true,
            ..Default::default()
        };

        let text = render_usage_compact(&info, false)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(text, "5h ▰▰▱▱▱▱▱▱▱▱ 77%\n7d ▰▰▰▰▰▰▰▱▱▱ 34%");
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}
