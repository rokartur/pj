//! Port of pi-hashline-edit-pro. `read` serves every line under a 4-letter anchor and
//! the edit tools address lines by anchor instead of by content or line number.
//! Anchors are unique across files, so edit tools need no path. A line that survives
//! an edit, or a change made outside pj, keeps its anchor; the anchor of a removed or
//! changed line is retired and later refused as stale.

use similar::{Algorithm, DiffOp, capture_diff_slices};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

/// Files whose anchors stay in memory; the least recently viewed one is dropped first.
const MAX_FILES: usize = 256;
const LETTERS: &[u8; 52] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BOM: char = '\u{feff}';

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Anchor([u8; 4]);

impl Anchor {
    pub fn parse(text: &str) -> Result<Self, String> {
        let invalid = || format!("`{text}` is not an anchor; anchors are 4 letters from read");
        let bytes: [u8; 4] = text.trim().as_bytes().try_into().map_err(|_| invalid())?;
        if !bytes.iter().all(u8::is_ascii_alphabetic) {
            return Err(invalid());
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for Anchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(std::str::from_utf8(&self.0).expect("anchors are ASCII letters"))
    }
}

/// File text split into lines, remembering BOM, line ending and final newline.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub lines: Vec<String>,
    bom: bool,
    crlf: bool,
    final_newline: bool,
}

impl Doc {
    /// An empty file is one empty line, so there is always a row to replace.
    pub fn parse(text: &str) -> Self {
        let bom = text.starts_with(BOM);
        let text = text.strip_prefix(BOM).unwrap_or(text);
        Self {
            lines: text_lines(text),
            bom,
            crlf: text.contains("\r\n"),
            final_newline: text.ends_with('\n'),
        }
    }

    pub fn render(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut text = String::new();
        if self.bom {
            text.push(BOM);
        }
        text.push_str(&self.lines.join(eol));
        if self.final_newline {
            text.push_str(eol);
        }
        text
    }

    fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty() && !self.final_newline
    }
}

/// Lines of model-written text. A trailing line break ends the last line instead of
/// adding a blank one, so `"\n"` is one blank line.
pub fn text_lines(text: &str) -> Vec<String> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

/// A file as last seen: its lines and the anchor of each.
#[derive(Clone, Debug)]
pub struct View {
    pub doc: Doc,
    pub anchors: Vec<Anchor>,
}

impl View {
    pub fn index(&self, anchor: Anchor) -> Option<usize> {
        self.anchors
            .iter()
            .position(|candidate| *candidate == anchor)
    }

    /// `anchor│line` rows for the lines in `range`.
    pub fn rows(&self, range: std::ops::Range<usize>) -> Vec<String> {
        range
            .map(|index| format!("{}│{}", self.anchors[index], self.doc.lines[index]))
            .collect()
    }

    fn kept(&self, range: std::ops::Range<usize>) -> Vec<Line> {
        range
            .map(|index| Line::Kept(self.anchors[index], self.doc.lines[index].clone()))
            .collect()
    }
}

/// One line of an edited file: an existing line keeps its anchor, a new one gets a fresh one.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Kept(Anchor, String),
    New(String),
}

struct Tracked {
    anchors: Vec<Anchor>,
    hashes: Vec<u64>,
}

/// Anchor bookkeeping for every viewed file.
#[derive(Default)]
pub struct Hashline {
    files: HashMap<PathBuf, Tracked>,
    owners: HashMap<Anchor, PathBuf>,
    /// Anchors of removed or changed lines, with where they last were.
    retired: HashMap<Anchor, (PathBuf, usize)>,
    recent: VecDeque<PathBuf>,
    undo: HashMap<PathBuf, Undo>,
}

/// The text a file had before and after the last hashline edit.
#[derive(Clone, Debug)]
pub struct Undo {
    pub before: String,
    pub after: String,
}

impl Hashline {
    /// The file and line index `anchor` had when last seen, live or retired.
    pub fn locate(&self, anchor: Anchor) -> Option<(&Path, usize)> {
        if let Some((path, index)) = self.retired.get(&anchor) {
            return Some((path, *index));
        }
        let path = self.owners.get(&anchor)?;
        let index = self
            .files
            .get(path)?
            .anchors
            .iter()
            .position(|a| *a == anchor)?;
        Some((path, index))
    }

    /// Anchors for `text`, the current content of `path`. Lines unchanged since the
    /// last view keep their anchors.
    pub fn view(&mut self, path: &Path, text: &str) -> View {
        let doc = Doc::parse(text);
        let hashes: Vec<u64> = doc.lines.iter().map(|line| hash_of(line)).collect();
        let mut lines: Vec<Line> = doc.lines.iter().cloned().map(Line::New).collect();
        if let Some(old) = self.files.get(path) {
            for op in capture_diff_slices(Algorithm::Myers, &old.hashes, &hashes) {
                if let DiffOp::Equal {
                    old_index,
                    new_index,
                    len,
                } = op
                {
                    for offset in 0..len {
                        let content = doc.lines[new_index + offset].clone();
                        lines[new_index + offset] =
                            Line::Kept(old.anchors[old_index + offset], content);
                    }
                }
            }
        }
        self.store(path, doc, lines)
    }

    /// Records `lines` as the new content of `path`, keeping the line ending style
    /// of `base`. Returns the new view; render its doc to get the text to write.
    pub fn commit(&mut self, path: &Path, base: &Doc, lines: Vec<Line>) -> View {
        let doc = Doc {
            lines: Vec::new(),
            final_newline: base.final_newline || base.is_empty(),
            ..base.clone()
        };
        if lines.is_empty() {
            let empty = Doc {
                final_newline: false,
                ..doc
            };
            return self.store(path, empty, vec![Line::New(String::new())]);
        }
        self.store(path, doc, lines)
    }

    pub fn remember_undo(&mut self, path: &Path, before: String, after: String) {
        self.undo.insert(path.to_path_buf(), Undo { before, after });
    }

    pub fn take_undo(&mut self, path: &Path) -> Option<Undo> {
        self.undo.remove(path)
    }

    /// Fills `doc.lines` from `lines`, assigns anchors and retires the ones that are gone.
    fn store(&mut self, path: &Path, mut doc: Doc, lines: Vec<Line>) -> View {
        let kept: HashSet<Anchor> = lines
            .iter()
            .filter_map(|line| match line {
                Line::Kept(anchor, _) => Some(*anchor),
                Line::New(_) => None,
            })
            .collect();
        if let Some(old) = self.files.remove(path) {
            for (index, anchor) in old.anchors.into_iter().enumerate() {
                if !kept.contains(&anchor) {
                    self.owners.remove(&anchor);
                    self.retired.insert(anchor, (path.to_path_buf(), index));
                }
            }
        }
        doc.lines.clear();
        let mut anchors = Vec::with_capacity(lines.len());
        for line in lines {
            let (anchor, content) = match line {
                Line::Kept(anchor, content) => (anchor, content),
                Line::New(content) => (self.fresh_anchor(path, &content), content),
            };
            self.owners.insert(anchor, path.to_path_buf());
            anchors.push(anchor);
            doc.lines.push(content);
        }
        let hashes = doc.lines.iter().map(|line| hash_of(line)).collect();
        self.files.insert(
            path.to_path_buf(),
            Tracked {
                anchors: anchors.clone(),
                hashes,
            },
        );
        self.touch(path);
        View { doc, anchors }
    }

    /// Derived from the path and line, so re-reading an unchanged file after a
    /// restart serves the same anchors.
    fn fresh_anchor(&self, path: &Path, content: &str) -> Anchor {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        content.hash(&mut hasher);
        let mut seed = hasher.finish();
        loop {
            let anchor = Anchor(std::array::from_fn(|position| {
                LETTERS[((seed >> (position * 16)) & 0xffff) as usize % LETTERS.len()]
            }));
            if !self.owners.contains_key(&anchor) && !self.retired.contains_key(&anchor) {
                return anchor;
            }
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
        }
    }

    fn touch(&mut self, path: &Path) {
        self.recent.retain(|recent| recent != path);
        self.recent.push_back(path.to_path_buf());
        while self.recent.len() > MAX_FILES {
            let Some(oldest) = self.recent.pop_front() else {
                break;
            };
            if let Some(tracked) = self.files.remove(&oldest) {
                for anchor in tracked.anchors {
                    self.owners.remove(&anchor);
                }
            }
        }
    }
}

fn hash_of(line: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

/// Replaces lines `range` with `new_lines`. Lines equal to a replaced one keep its anchor.
pub fn replace_range(view: &View, range: RangeInclusive<usize>, new_lines: &[String]) -> Vec<Line> {
    let (start, end) = (*range.start(), *range.end());
    let old = &view.doc.lines[range];
    let mut lines = view.kept(0..start);
    for op in capture_diff_slices(Algorithm::Myers, old, new_lines) {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for offset in 0..len {
                    lines.push(Line::Kept(
                        view.anchors[start + old_index + offset],
                        new_lines[new_index + offset].clone(),
                    ));
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            }
            | DiffOp::Replace {
                new_index, new_len, ..
            } => lines.extend(
                new_lines[new_index..new_index + new_len]
                    .iter()
                    .cloned()
                    .map(Line::New),
            ),
            DiffOp::Delete { .. } => {}
        }
    }
    lines.extend(view.kept(end + 1..view.anchors.len()));
    lines
}

/// Replaces every `old` inside lines `range` with `new`.
pub fn replace_match(
    view: &View,
    range: RangeInclusive<usize>,
    old: &str,
    new: &str,
) -> Result<Vec<Line>, String> {
    if old.is_empty() {
        return Err("old_string must not be empty".into());
    }
    let text = view.doc.lines[range.clone()].join("\n");
    if !text.contains(old) {
        return Err(format!(
            "old_string {old:?} does not occur in the selected lines"
        ));
    }
    let replaced: Vec<String> = text
        .replace(old, new)
        .split('\n')
        .map(String::from)
        .collect();
    Ok(replace_range(view, range, &replaced))
}

/// Inserts `new_lines` after or before line `index`.
pub fn insert(view: &View, index: usize, after: bool, new_lines: &[String]) -> Vec<Line> {
    let at = if after { index + 1 } else { index };
    let mut lines = view.kept(0..at);
    lines.extend(new_lines.iter().cloned().map(Line::New));
    lines.extend(view.kept(at..view.anchors.len()));
    lines
}

/// Removes lines `range`.
pub fn remove(view: &View, range: RangeInclusive<usize>) -> Vec<Line> {
    let mut lines = view.kept(0..*range.start());
    lines.extend(view.kept(range.end() + 1..view.anchors.len()));
    lines
}

/// Moves lines `range` next to line `target` of the same file; moved lines keep their anchors.
pub fn move_within(
    view: &View,
    range: RangeInclusive<usize>,
    target: usize,
    after: bool,
) -> Result<Vec<Line>, String> {
    if range.contains(&target) {
        return Err("the destination must be outside the moved lines".into());
    }
    let moved = view.kept(*range.start()..range.end() + 1);
    let at = if after { target + 1 } else { target };
    let mut lines = Vec::with_capacity(view.anchors.len());
    for (index, line) in view.kept(0..view.anchors.len()).into_iter().enumerate() {
        if index == at {
            lines.extend(moved.iter().cloned());
        }
        if !range.contains(&index) {
            lines.push(line);
        }
    }
    if at == view.anchors.len() {
        lines.extend(moved);
    }
    Ok(lines)
}

/// `-anchor│old` / `+anchor│new` rows with one line of context; `...` marks skipped lines.
pub fn render_diff(old: &View, new: &View) -> String {
    let mut rows: Vec<(char, String)> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &old.anchors, &new.anchors) {
        let (old_range, new_range) = (op.old_range(), op.new_range());
        if let DiffOp::Equal { .. } = op {
            rows.extend(new.rows(new_range).into_iter().map(|row| (' ', row)));
            continue;
        }
        rows.extend(old.rows(old_range).into_iter().map(|row| ('-', row)));
        rows.extend(new.rows(new_range).into_iter().map(|row| ('+', row)));
    }
    let changed: Vec<usize> = (0..rows.len()).filter(|&row| rows[row].0 != ' ').collect();
    if changed.is_empty() {
        return "No changes made".into();
    }
    let near_change = |row: usize| changed.iter().any(|&change| change.abs_diff(row) <= 1);
    let mut out = Vec::new();
    let mut skipped = false;
    for (index, (tag, row)) in rows.iter().enumerate() {
        if !near_change(index) {
            skipped = true;
            continue;
        }
        if skipped || (index > 0 && out.is_empty()) {
            out.push(" ...".to_string());
        }
        skipped = false;
        out.push(format!("{tag}{row}"));
    }
    if skipped {
        out.push(" ...".to_string());
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> &'static Path {
        Path::new("/repo/a.rs")
    }

    fn view_of(store: &mut Hashline, text: &str) -> View {
        store.view(path(), text)
    }

    fn lines(text: &str) -> Vec<String> {
        text_lines(text)
    }

    #[test]
    fn untouched_lines_keep_anchors_across_outside_edits() {
        let mut store = Hashline::default();
        let before = view_of(&mut store, "a\nb\nc\n");
        let after = view_of(&mut store, "a\nnew\nb\nc\n");
        assert_eq!(after.anchors[0], before.anchors[0]);
        assert_eq!(after.anchors[2], before.anchors[1]);
        assert_eq!(after.anchors[3], before.anchors[2]);
        assert!(!before.anchors.contains(&after.anchors[1]));
    }

    #[test]
    fn a_changed_line_retires_its_anchor() {
        let mut store = Hashline::default();
        let before = view_of(&mut store, "a\nb\n");
        let after = view_of(&mut store, "a\nB\n");
        assert_eq!(after.index(before.anchors[1]), None);
        assert_eq!(store.locate(before.anchors[1]), Some((path(), 1)));
        assert_eq!(store.locate(after.anchors[1]), Some((path(), 1)));
        assert_ne!(after.anchors[1], before.anchors[1]);
    }

    #[test]
    fn same_text_gets_the_same_anchors_after_a_restart() {
        let first = view_of(&mut Hashline::default(), "x\ny\n");
        let again = view_of(&mut Hashline::default(), "x\ny\n");
        assert_eq!(first.anchors, again.anchors);
    }

    #[test]
    fn replace_keeps_anchors_of_equal_lines_and_preserves_crlf() {
        let mut store = Hashline::default();
        let view = view_of(&mut store, "a\r\nb\r\nc\r\nd\r\n");
        let edited = replace_range(&view, 1..=2, &lines("b\nC\nextra"));
        let new = store.commit(path(), &view.doc, edited);
        assert_eq!(new.doc.render(), "a\r\nb\r\nC\r\nextra\r\nd\r\n");
        assert_eq!(new.anchors[1], view.anchors[1]);
        assert_eq!(
            render_diff(&view, &new),
            format!(
                " ...\n {}│b\n-{}│c\n+{}│C\n+{}│extra\n {}│d",
                view.anchors[1], view.anchors[2], new.anchors[2], new.anchors[3], view.anchors[3]
            )
        );
    }

    #[test]
    fn deleting_every_line_leaves_one_empty_row() {
        let mut store = Hashline::default();
        let view = view_of(&mut store, "a\nb\n");
        let new = store.commit(path(), &view.doc, replace_range(&view, 0..=1, &[]));
        assert_eq!(new.doc.render(), "");
        assert_eq!(new.doc.lines, [""]);
        let seeded = replace_range(&new, 0..=0, &lines("first\n"));
        assert_eq!(
            store.commit(path(), &new.doc, seeded).doc.render(),
            "first\n"
        );
    }

    #[test]
    fn replace_match_rewrites_every_occurrence_in_range_only() {
        let mut store = Hashline::default();
        let view = view_of(&mut store, "x = 1\nx = 2\nx = 3\n");
        let edited = replace_match(&view, 0..=1, "x", "y").unwrap();
        let new = store.commit(path(), &view.doc, edited);
        assert_eq!(new.doc.render(), "y = 1\ny = 2\nx = 3\n");
        assert!(replace_match(&view, 2..=2, "zz", "y").is_err());
    }

    #[test]
    fn insert_and_move_place_lines_around_the_target() {
        let mut store = Hashline::default();
        let view = view_of(&mut store, "a\nb\nc\n");
        let inserted = store.commit(path(), &view.doc, insert(&view, 0, true, &lines("\n")));
        assert_eq!(inserted.doc.render(), "a\n\nb\nc\n");
        let moved = move_within(&inserted, 0..=1, 3, true).unwrap();
        let moved = store.commit(path(), &inserted.doc, moved);
        assert_eq!(moved.doc.render(), "b\nc\na\n\n");
        assert_eq!(moved.anchors[2], view.anchors[0]);
        assert!(move_within(&inserted, 0..=1, 1, true).is_err());
    }

    #[test]
    fn diff_marks_skipped_lines() {
        let mut store = Hashline::default();
        let view = view_of(&mut store, "1\n2\n3\n4\n5\n");
        let new = store.commit(path(), &view.doc, remove(&view, 2..=2));
        let diff = render_diff(&view, &new);
        assert!(diff.starts_with(" ...\n "), "{diff}");
        assert!(diff.ends_with("\n ..."), "{diff}");
        assert_eq!(diff.lines().count(), 5);
    }

    #[test]
    fn anchors_must_be_four_letters() {
        assert!(Anchor::parse("AbCd").is_ok());
        assert!(Anchor::parse("Ab1d").is_err());
        assert!(Anchor::parse("Abc").is_err());
    }
}
