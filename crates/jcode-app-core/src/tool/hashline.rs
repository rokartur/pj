//! Anchor-addressed edit tools ported from pi-hashline-edit-pro: `replace`,
//! `replace_match`, `insert`, `move`, `copy`, `undo_last_change`. `read` serves the anchors.

use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, FileOp, FileTouch};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use pj_ext::hashline::{self, Anchor, Hashline, Line, View};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

/// Rows shown around a stale anchor so the model can retry without a new read.
const STALE_CONTEXT: usize = 5;

// shortcut: one store per process, shared by all sessions; anchors are unique across
// files, so sessions only see each other's anchors. Per-session stores if that matters.
static STORE: LazyLock<Mutex<Hashline>> = LazyLock::new(Mutex::default);

pub(super) fn store() -> MutexGuard<'static, Hashline> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct ReplaceTool;
pub struct ReplaceMatchTool;
pub struct InsertTool;
pub struct MoveTool;
pub struct CopyTool;
pub struct UndoTool;

#[derive(Deserialize)]
struct ReplaceInput {
    #[serde(alias = "replace_from")]
    remove_from: String,
    #[serde(alias = "replace_to")]
    remove_to: String,
    text: String,
}

#[derive(Deserialize)]
struct ReplaceMatchInput {
    #[serde(alias = "remove_from")]
    replace_from: String,
    #[serde(alias = "remove_to")]
    replace_to: String,
    old_string: String,
    new_string: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Direction {
    After,
    Before,
}

#[derive(Deserialize)]
struct InsertInput {
    anchor: String,
    direction: Direction,
    text: String,
}

#[derive(Deserialize)]
struct RelocateInput {
    source_from: String,
    source_to: String,
    anchor: String,
    direction: Direction,
}

#[derive(Deserialize)]
struct UndoInput {
    path: String,
}

fn anchor_property(what: &str) -> Value {
    json!({"type": "string", "description": format!("4-char anchor of {what}, from read.")})
}

fn direction_property() -> Value {
    json!({"type": "string", "enum": ["after", "before"]})
}

#[async_trait]
impl Tool for ReplaceTool {
    fn name(&self) -> &str {
        "replace"
    }

    fn description(&self) -> &str {
        "Replace lines remove_from..remove_to (anchors from read) with text. \"\" deletes."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["remove_from", "remove_to", "text"],
            "properties": {
                "remove_from": anchor_property("the first line"),
                "remove_to": anchor_property("the last line"),
                "text": {"type": "string", "description": "Replacement lines. A trailing newline adds no blank line."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: ReplaceInput = serde_json::from_value(input)?;
        let new_lines = if input.text.is_empty() {
            Vec::new()
        } else {
            hashline::text_lines(&input.text)
        };
        edit_range(&ctx, &input.remove_from, &input.remove_to, |view, range| {
            Ok(hashline::replace_range(view, range, &new_lines))
        })
        .await
    }
}

#[async_trait]
impl Tool for ReplaceMatchTool {
    fn name(&self) -> &str {
        "replace_match"
    }

    fn description(&self) -> &str {
        "Replace every old_string with new_string inside lines replace_from..replace_to."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["replace_from", "replace_to", "old_string", "new_string"],
            "properties": {
                "replace_from": anchor_property("the first line"),
                "replace_to": anchor_property("the last line"),
                "old_string": {"type": "string", "description": "Exact text to find in those lines."},
                "new_string": {"type": "string", "description": "Replacement for every match."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: ReplaceMatchInput = serde_json::from_value(input)?;
        edit_range(
            &ctx,
            &input.replace_from,
            &input.replace_to,
            |view, range| {
                hashline::replace_match(view, range, &input.old_string, &input.new_string)
                    .map_err(|error| anyhow!(error))
            },
        )
        .await
    }
}

#[async_trait]
impl Tool for InsertTool {
    fn name(&self) -> &str {
        "insert"
    }

    fn description(&self) -> &str {
        "Insert text after or before the line at anchor."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["anchor", "direction", "text"],
            "properties": {
                "anchor": anchor_property("the line to insert next to"),
                "direction": direction_property(),
                "text": {"type": "string", "description": "Lines to insert. \"\" inserts one blank line."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: InsertInput = serde_json::from_value(input)?;
        let new_lines = hashline::text_lines(&input.text);
        let after = input.direction == Direction::After;
        edit_range(&ctx, &input.anchor, &input.anchor, |view, range| {
            Ok(hashline::insert(view, *range.start(), after, &new_lines))
        })
        .await
    }
}

fn relocate_schema() -> Value {
    json!({
        "type": "object",
        "required": ["source_from", "source_to", "anchor", "direction"],
        "properties": {
            "source_from": anchor_property("the first source line"),
            "source_to": anchor_property("the last source line"),
            "anchor": anchor_property("the destination line, in this or another file"),
            "direction": direction_property()
        }
    })
}

#[async_trait]
impl Tool for MoveTool {
    fn name(&self) -> &str {
        "move"
    }

    fn description(&self) -> &str {
        "Move lines source_from..source_to after or before anchor, in this or another file."
    }

    fn parameters_schema(&self) -> Value {
        relocate_schema()
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        relocate(serde_json::from_value(input)?, &ctx, true).await
    }
}

#[async_trait]
impl Tool for CopyTool {
    fn name(&self) -> &str {
        "copy"
    }

    fn description(&self) -> &str {
        "Copy lines source_from..source_to after or before anchor, in this or another file."
    }

    fn parameters_schema(&self) -> Value {
        relocate_schema()
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        relocate(serde_json::from_value(input)?, &ctx, false).await
    }
}

#[async_trait]
impl Tool for UndoTool {
    fn name(&self) -> &str {
        "undo_last_change"
    }

    fn description(&self) -> &str {
        "Undo the last anchor edit on a file, unless the file changed since."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string", "description": "File to restore."}}
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: UndoInput = serde_json::from_value(input)?;
        let path = ctx.resolve_path(Path::new(&input.path));
        let _lock = super::file_lock::lock(&path).await;
        let mut store = store();
        let Some(undo) = store.take_undo(&path) else {
            bail!("No anchor edit to undo in {}", input.path);
        };
        // A file deleted since the edit is restored; one edited since is left alone.
        let current = std::fs::read_to_string(&path).ok();
        if current
            .as_ref()
            .is_some_and(|current| *current != undo.after)
        {
            let message = format!(
                "{} changed since the last edit; undo refused, file untouched",
                input.path
            );
            store.remember_undo(&path, undo.before, undo.after);
            bail!(message);
        }
        let before = store.view(&path, current.as_deref().unwrap_or_default());
        std::fs::write(&path, &undo.before)?;
        let after = store.view(&path, &undo.before);
        publish(&ctx, &path, "undid the last edit");
        Ok(ToolOutput::new(diff_report(&path, &before, &after)))
    }
}

/// Resolves both anchors in their file as it is on disk now, applies `edit` to the
/// line range between them and writes the result.
async fn edit_range(
    ctx: &ToolContext,
    from: &str,
    to: &str,
    edit: impl FnOnce(&View, std::ops::RangeInclusive<usize>) -> Result<Vec<Line>>,
) -> Result<ToolOutput> {
    let path = owner_path(from)?;
    let _lock = super::file_lock::lock(&path).await;
    let mut store = store();
    let (view, indexes) = fresh_view(&mut store, &path, &[from, to])?;
    let (start, end) = (indexes[0], indexes[1]);
    if start > end {
        bail!("{from} comes after {to}; pass the first line first");
    }
    let lines = edit(&view, start..=end)?;
    let report = commit(&mut store, ctx, &path, &view, lines)?;
    Ok(ToolOutput::new(report))
}

async fn relocate(
    input: RelocateInput,
    ctx: &ToolContext,
    remove_source: bool,
) -> Result<ToolOutput> {
    let source = owner_path(&input.source_from)?;
    let destination = owner_path(&input.anchor)?;
    let after = input.direction == Direction::After;
    let _source_lock = super::file_lock::lock(&source).await;
    let _destination_lock = if destination == source {
        None
    } else {
        Some(super::file_lock::lock(&destination).await)
    };
    let mut store = store();
    if destination == source {
        let (view, indexes) = fresh_view(
            &mut store,
            &source,
            &[&input.source_from, &input.source_to, &input.anchor],
        )?;
        let range = ordered_range(&input, indexes[0], indexes[1])?;
        let lines = if remove_source {
            hashline::move_within(&view, range, indexes[2], after)
                .map_err(|error| anyhow!(error))?
        } else {
            let copied = view.doc.lines[range].to_vec();
            hashline::insert(&view, indexes[2], after, &copied)
        };
        return Ok(ToolOutput::new(commit(
            &mut store, ctx, &source, &view, lines,
        )?));
    }
    let (source_view, indexes) =
        fresh_view(&mut store, &source, &[&input.source_from, &input.source_to])?;
    let range = ordered_range(&input, indexes[0], indexes[1])?;
    let (destination_view, target) = fresh_view(&mut store, &destination, &[&input.anchor])?;
    let copied = source_view.doc.lines[range.clone()].to_vec();
    let inserted = hashline::insert(&destination_view, target[0], after, &copied);
    let mut report = commit(&mut store, ctx, &destination, &destination_view, inserted)?;
    if remove_source {
        let removed = hashline::remove(&source_view, range);
        report.push_str("\n\n");
        report.push_str(&commit(&mut store, ctx, &source, &source_view, removed)?);
    }
    Ok(ToolOutput::new(report))
}

fn ordered_range(
    input: &RelocateInput,
    start: usize,
    end: usize,
) -> Result<std::ops::RangeInclusive<usize>> {
    if start > end {
        bail!(
            "{} comes after {}; pass the first line first",
            input.source_from,
            input.source_to
        );
    }
    Ok(start..=end)
}

fn owner_path(anchor: &str) -> Result<PathBuf> {
    let anchor = Anchor::parse(anchor).map_err(|error| anyhow!(error))?;
    store()
        .locate(anchor)
        .map(|(path, _)| path.to_path_buf())
        .ok_or_else(|| anyhow!("Anchor {anchor} was never served; read the file first"))
}

/// Re-reads `path` and finds each anchor in it. A missing anchor is refused with the
/// current rows around where it used to be, carrying fresh anchors.
fn fresh_view(store: &mut Hashline, path: &Path, anchors: &[&str]) -> Result<(View, Vec<usize>)> {
    let mut parsed = Vec::with_capacity(anchors.len());
    for anchor in anchors {
        let anchor = Anchor::parse(anchor).map_err(|error| anyhow!(error))?;
        let last_index = store.locate(anchor).map(|(_, index)| index);
        parsed.push((anchor, last_index));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| anyhow!("cannot read {}: {error}", path.display()))?;
    let view = store.view(path, &text);
    let mut indexes = Vec::with_capacity(parsed.len());
    for (anchor, last_index) in parsed {
        if let Some(index) = view.index(anchor) {
            indexes.push(index);
            continue;
        }
        let Some(last_index) = last_index else {
            bail!(
                "Anchor {anchor} is not in {}; read the file again",
                path.display()
            );
        };
        let start = last_index.saturating_sub(STALE_CONTEXT);
        let end = (last_index + STALE_CONTEXT + 1).min(view.anchors.len());
        bail!(
            "Anchor {anchor} is stale: its line in {} changed. Current rows:\n{}",
            path.display(),
            view.rows(start.min(end)..end).join("\n")
        );
    }
    Ok((view, indexes))
}

fn commit(
    store: &mut Hashline,
    ctx: &ToolContext,
    path: &Path,
    before: &View,
    lines: Vec<Line>,
) -> Result<String> {
    let before_text = before.doc.render();
    let after = store.commit(path, &before.doc, lines);
    let after_text = after.doc.render();
    if after_text == before_text {
        return Ok(format!("{}: No changes made", path.display()));
    }
    std::fs::write(path, &after_text)?;
    store.remember_undo(path, before_text, after_text);
    publish(ctx, path, "edited by anchor");
    Ok(diff_report(path, before, &after))
}

fn diff_report(path: &Path, before: &View, after: &View) -> String {
    format!(
        "{}\n{}",
        path.display(),
        hashline::render_diff(before, after)
    )
}

fn publish(ctx: &ToolContext, path: &Path, summary: &str) {
    Bus::global().publish(BusEvent::FileTouch(FileTouch {
        session_id: ctx.session_id.clone(),
        path: path.to_path_buf(),
        op: FileOp::Edit,
        intent: None,
        summary: Some(summary.to_string()),
        detail: None,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::read::ReadTool;

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            session_id: "s".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: Some(dir.to_path_buf()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }

    async fn anchors(dir: &Path, file: &str) -> Vec<String> {
        let read = ReadTool::new()
            .execute(json!({"file_path": file}), ctx(dir))
            .await
            .unwrap();
        read.output
            .lines()
            .map(|row| row.split('│').next().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn edits_by_anchor_refuses_stale_anchors_and_undoes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let rows = anchors(dir.path(), "a.txt").await;

        ReplaceTool
            .execute(
                json!({"remove_from": rows[1], "remove_to": rows[1], "text": "TWO"}),
                ctx(dir.path()),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\nTWO\nthree\n");

        let stale = InsertTool
            .execute(
                json!({"anchor": rows[1], "direction": "after", "text": "x"}),
                ctx(dir.path()),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(stale.contains("is stale"), "{stale}");
        assert!(stale.contains(&format!("{}│one", rows[0])), "{stale}");

        MoveTool
            .execute(
                json!({"source_from": rows[0], "source_to": rows[0], "anchor": rows[2], "direction": "after"}),
                ctx(dir.path()),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "TWO\nthree\none\n");

        UndoTool
            .execute(json!({"path": "a.txt"}), ctx(dir.path()))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\nTWO\nthree\n");
    }
}
