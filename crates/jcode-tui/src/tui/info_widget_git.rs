//! Changes widget: one line of working-tree and upstream counts.

use super::frame::{self, Framed};
use super::{GitInfo, InfoWidgetData};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
/// Paths an edit-style tool call wrote to, as given in its input (absolute or
/// relative to the session working directory). Non-edit tools yield nothing.
pub(crate) fn edited_paths_from_tool_call(name: &str, input: &serde_json::Value) -> Vec<String> {
    if !crate::tui::ui::tools_ui::is_edit_tool_name(name) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for key in ["file_path", "path"] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str()) {
            out.push(p.to_string());
        }
    }
    if let Some(text) = input
        .get("patch_text")
        .or_else(|| input.get("patch"))
        .and_then(|v| v.as_str())
    {
        for line in text.lines() {
            let t = line.trim();
            let codex = t
                .strip_prefix("*** Update File: ")
                .or_else(|| t.strip_prefix("*** Add File: "))
                .or_else(|| t.strip_prefix("*** Move to: "));
            let unified = line
                .strip_prefix("+++ ")
                .map(|r| r.split('\t').next().unwrap_or(r))
                .filter(|r| *r != "/dev/null")
                .map(|r| r.strip_prefix("b/").unwrap_or(r));
            if let Some(p) = codex.or(unified) {
                out.push(p.trim().to_string());
            }
        }
    }
    out
}

/// Resolve an edited path to an absolute, lexically normalized path.
/// Relative paths are taken relative to the session working directory.
pub(crate) fn resolve_edited_path(path: &str, working_dir: Option<&str>) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(wd) = working_dir {
        std::path::Path::new(wd).join(p)
    } else {
        p.to_path_buf()
    };
    let mut out = std::path::PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `staged:2 unstaged:5 +760 −380 ↑2 ↓1`, zero counts omitted. `None` when the
/// tree is clean and in sync with its upstream.
pub(super) fn changes_line(info: &GitInfo) -> Option<Line<'static>> {
    let green = rgb(90, 170, 100);
    let muted = rgb(130, 130, 145);
    let badges = [
        ("staged:", info.staged, green),
        (
            "unstaged:",
            info.modified + info.untracked,
            rgb(240, 200, 80),
        ),
        ("+", info.added_total, green),
        ("−", info.removed_total, rgb(200, 100, 95)),
        ("↑", info.ahead, muted),
        ("↓", info.behind, muted),
    ];
    let mut spans = Vec::new();
    for (label, count, color) in badges {
        if count == 0 {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("{label}{count}"),
            Style::default().fg(color),
        ));
    }
    (!spans.is_empty()).then(|| Line::from(spans))
}

pub(super) fn changes_has_data(info: &GitInfo) -> bool {
    changes_line(info).is_some()
}

pub(super) fn changes_height(info: &GitInfo) -> u16 {
    u16::from(changes_has_data(info))
}

pub(super) fn render_changes_framed(data: &InfoWidgetData) -> Framed {
    let Some(line) = data.git_info.as_ref().and_then(changes_line) else {
        return Framed::default();
    };
    Framed::body(vec![line]).title(frame::label("Changes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn changes_line_shows_only_nonzero_counts_in_order() {
        let git = GitInfo {
            branch: "main".into(),
            staged: 2,
            modified: 3,
            untracked: 1,
            added_total: 760,
            removed_total: 380,
            ahead: 2,
            behind: 1,
            ..Default::default()
        };
        let line = changes_line(&git).expect("dirty tree");
        assert_eq!(text(&line), "staged:2 unstaged:4 +760 −380 ↑2 ↓1");
        let ahead_only = GitInfo {
            ahead: 3,
            ..Default::default()
        };
        assert_eq!(text(&changes_line(&ahead_only).unwrap()), "↑3");
    }

    #[test]
    fn clean_tree_in_sync_has_no_changes_widget() {
        let clean = GitInfo {
            branch: "main".into(),
            ..Default::default()
        };
        assert!(!changes_has_data(&clean));
        assert_eq!(changes_height(&clean), 0);
    }

    #[test]
    fn porcelain_pairs_collapse_to_one_letter() {
        use crate::tui::app::helpers::porcelain_status_letter as l;
        assert_eq!(l(b'?', b'?'), '?');
        assert_eq!(l(b' ', b'M'), 'M');
        assert_eq!(l(b'M', b' '), 'M');
        assert_eq!(l(b'A', b' '), 'A');
        assert_eq!(l(b'A', b'M'), 'M');
        assert_eq!(l(b' ', b'D'), 'D');
        assert_eq!(l(b'R', b' '), 'R');
        assert_eq!(l(b'U', b'U'), 'U');
        assert_eq!(l(b'A', b'A'), 'U');
    }

    #[test]
    fn edited_paths_cover_edit_write_and_both_patch_formats() {
        use serde_json::json;
        assert_eq!(
            edited_paths_from_tool_call("edit", &json!({"file_path": "/r/a.rs"})),
            ["/r/a.rs"]
        );
        assert_eq!(
            edited_paths_from_tool_call("write", &json!({"file_path": "b.rs"})),
            ["b.rs"]
        );
        let codex = "*** Begin Patch\n*** Update File: x/c.rs\n@@\n-a\n+b\n*** Add File: d.rs\n+x\n*** End Patch";
        assert_eq!(
            edited_paths_from_tool_call("apply_patch", &json!({"patch_text": codex})),
            ["x/c.rs", "d.rs"]
        );
        let unified = "--- a/e.rs\n+++ b/e.rs\n@@\n-a\n+b\n";
        assert_eq!(
            edited_paths_from_tool_call("apply_patch", &json!({"patch_text": unified})),
            ["e.rs"]
        );
        assert!(edited_paths_from_tool_call("read", &json!({"file_path": "a.rs"})).is_empty());
        assert!(edited_paths_from_tool_call("bash", &json!({"command": "rm a"})).is_empty());
    }

    #[test]
    fn edited_paths_resolve_against_working_dir() {
        let p = resolve_edited_path("../src/./a.rs", Some("/home/me/jcode/crates"));
        assert_eq!(p, std::path::PathBuf::from("/home/me/jcode/src/a.rs"));
    }

    #[test]
    fn numstat_parses_plain_binary_and_renamed_paths() {
        let m = crate::tui::app::helpers::parse_numstat(
            "84\t12\tsrc/a.rs\n-\t-\tlogo.png\n3\t1\tsrc/{old => new}/b.rs\n2\t2\told.rs => new.rs\n",
        );
        assert_eq!(m["src/a.rs"], (Some(84), Some(12)));
        assert_eq!(m["logo.png"], (None, None));
        assert_eq!(m["src/new/b.rs"], (Some(3), Some(1)));
        assert_eq!(m["new.rs"], (Some(2), Some(2)));
    }

    /// Real-repo probe: runs the production git gather against the working
    /// directory and renders the Changes widget. Run on demand with
    /// `cargo test -p jcode-tui -- --ignored real_repo_changes --nocapture`.
    #[test]
    #[ignore]
    fn real_repo_changes_widget() {
        let info = crate::tui::app::helpers::gather_git_info_inner().expect("inside a git repo");
        println!("{:?}", changes_line(&info));
        assert!(info.repo_root.is_some());
        let ordered: Vec<_> = info.dirty_files.iter().map(|f| f.modified_at).collect();
        assert!(ordered.windows(2).all(|w| w[0] >= w[1]), "newest first");
    }
}
