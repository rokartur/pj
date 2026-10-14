//! Port of pi-condense: after a turn, a cheap model summarizes that turn's tool
//! results; the provider then sees the summary in place of the raw outputs, and
//! `context_tree_query` brings any original back. The transcript keeps the originals.
//!
//! shortcut: pi's chain compression, error purging, content-hash dedup and spill
//! files are not ported; add them if long sessions still hit the context limit.

use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CondenseConfig {
    pub enabled: bool,
    pub summarizer_model: String,
    /// A turn whose tool outputs total fewer characters is left as is.
    pub min_batch_chars: usize,
    pub protected_tools: Vec<String>,
    /// Globs for files whose `read` output is never condensed, e.g. skills.
    pub protected_paths: Vec<String>,
}

impl Default for CondenseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            summarizer_model: "anthropic/claude-haiku-4-5".into(),
            min_batch_chars: 1000,
            protected_tools: Vec::new(),
            protected_paths: vec!["**/skills/**/*.md".into()],
        }
    }
}

/// One finished tool call from the turn being condensed.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub output: String,
}

impl CondenseConfig {
    /// The calls worth summarizing, or nothing when the turn is too small.
    pub fn select(&self, calls: Vec<ToolCall>) -> Vec<ToolCall> {
        let selected: Vec<ToolCall> = calls
            .into_iter()
            .filter(|call| !self.is_protected(call))
            .collect();
        let total: usize = selected.iter().map(|call| call.output.len()).sum();
        if !self.enabled || total < self.min_batch_chars {
            return Vec::new();
        }
        selected
    }

    fn is_protected(&self, call: &ToolCall) -> bool {
        // A restored original or added instructions must reach the model verbatim.
        let always_protected = ["context_tree_query", "add_directory"];
        if always_protected.contains(&call.name.as_str())
            || self.protected_tools.contains(&call.name)
        {
            return true;
        }
        let Some(path) = call.input.get("file_path").and_then(|path| path.as_str()) else {
            return false;
        };
        self.protected_paths
            .iter()
            .any(|pattern| glob::Pattern::new(pattern).is_ok_and(|pattern| pattern.matches(path)))
    }
}

pub const SUMMARIZER_SYSTEM: &str = "You condense an AI coding agent's tool results so the agent \
can keep working without the raw output. For each call write the ref in backticks, one line on \
what it did and its outcome, then bullets with the facts the agent will need: exact file paths, \
line numbers, identifiers, error messages, versions and numbers, copied verbatim. Drop \
boilerplate and anything the agent will not need. Never invent facts. Plain Markdown, no preamble.";

/// Each output is cut to this many characters before it reaches the summarizer.
const MAX_OUTPUT_CHARS_PER_CALL: usize = 20_000;

pub fn summarizer_prompt(calls: &[ToolCall]) -> String {
    let mut prompt = String::new();
    for call in calls {
        let mut end = call.output.len().min(MAX_OUTPUT_CHARS_PER_CALL);
        while !call.output.is_char_boundary(end) {
            end -= 1;
        }
        prompt.push_str(&format!(
            "<call ref=\"{}\" tool=\"{}\">\n<input>{}</input>\n<output>\n{}\n</output>\n</call>\n",
            call.id,
            call.name,
            call.input,
            &call.output[..end]
        ));
    }
    prompt
}

/// What the provider sees instead of each output: the first call carries the
/// summary, the others point at it.
pub fn stubs(calls: &[ToolCall], summary: &str) -> Vec<(String, String)> {
    let ids: Vec<&str> = calls.iter().map(|call| call.id.as_str()).collect();
    let refs = serde_json::to_string(&ids).expect("strings serialize");
    let Some((first, rest)) = calls.split_first() else {
        return Vec::new();
    };
    let mut stubs = vec![(
        first.id.clone(),
        format!(
            "{}\n\n[Condensed tool results. Originals: context_tree_query({{\"toolCallIds\": {refs}}})]",
            summary.trim()
        ),
    )];
    for call in rest {
        stubs.push((
            call.id.clone(),
            format!(
                "[Condensed into the summary in tool result {}. Original: context_tree_query({{\"toolCallIds\": [\"{}\"]}})]",
                first.id, call.id
            ),
        ));
    }
    stubs
}

/// Stubs for one session, kept next to the session so a resumed session sees the same context.
pub fn load_stubs(path: &Path) -> Result<HashMap<String, String>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

pub fn save_stubs(path: &Path, stubs: &HashMap<String, String>) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let json = serde_json::to_vec(stubs).expect("map serializes");
    std::fs::write(path, json).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str, input: serde_json::Value, output_len: usize) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            input,
            output: "x".repeat(output_len),
        }
    }

    #[test]
    fn skips_small_turns_and_protected_reads() {
        let config = CondenseConfig::default();
        assert!(
            config
                .select(vec![call("a", "bash", json!({}), 999)])
                .is_empty()
        );
        let skill = json!({"file_path": "/home/a/.pi/skills/tdd/SKILL.md"});
        let selected = config.select(vec![
            call("a", "read", skill, 5000),
            call("b", "bash", json!({}), 1000),
        ]);
        let ids: Vec<&str> = selected.iter().map(|call| call.id.as_str()).collect();
        assert_eq!(ids, ["b"]);
    }

    #[test]
    fn first_stub_carries_the_summary_and_every_stub_names_its_ref() {
        let calls = [
            call("a", "bash", json!({}), 1),
            call("b", "rg", json!({}), 1),
        ];
        let stubs = stubs(&calls, "- did things\n");
        assert_eq!(
            stubs[0].1,
            "- did things\n\n[Condensed tool results. Originals: context_tree_query({\"toolCallIds\": [\"a\",\"b\"]})]"
        );
        assert!(stubs[1].1.contains("summary in tool result a"));
        assert!(stubs[1].1.contains("[\"b\"]"));
    }

    #[test]
    fn stubs_round_trip_through_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("condense/s.json");
        assert!(load_stubs(&path).unwrap().is_empty());
        let stubs = HashMap::from([("a".to_string(), "stub".to_string())]);
        save_stubs(&path, &stubs).unwrap();
        assert_eq!(load_stubs(&path).unwrap(), stubs);
    }
}
