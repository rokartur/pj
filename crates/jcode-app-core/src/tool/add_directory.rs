//! `add_directory`, ported from pi-add-dir: brings the instructions and skills of a
//! directory outside the working directory into the conversation.
//!
//! shortcut: the result lives in the transcript only. pi-add-dir's `/dir-*` commands,
//! persistent scopes and `search_external_files` are not ported; `fd` and `rg` take a path.

use super::{Tool, ToolContext, ToolOutput};
use crate::skill::SkillRegistry;
use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

const INSTRUCTION_FILES: [&str; 5] = [
    ".agents/AGENTS.md",
    "AGENTS.md",
    "CLAUDE.md",
    ".pi/AGENTS.md",
    ".pi/CLAUDE.md",
];

pub struct AddDirectoryTool;

#[derive(Deserialize)]
struct Input {
    path: String,
}

#[async_trait]
impl Tool for AddDirectoryTool {
    fn name(&self) -> &str {
        "add_directory"
    }

    fn description(&self) -> &str {
        "Load AGENTS.md, CLAUDE.md and skills of an outside directory."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "intent": super::intent_schema_property(),
                "path": {"type": "string", "description": "Directory to add; ~ is expanded."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: Input = serde_json::from_value(input)?;
        let expanded = match input.path.strip_prefix("~/") {
            Some(rest) => {
                dirs::home_dir().map_or_else(|| input.path.clone().into(), |home| home.join(rest))
            }
            None => input.path.clone().into(),
        };
        let dir = ctx.resolve_path(&expanded);
        if !dir.is_dir() {
            bail!("{} is not a directory", dir.display());
        }
        let dir = dir.canonicalize()?;
        Ok(ToolOutput::new(describe(&dir)?).with_title(format!("added {}", dir.display())))
    }
}

fn describe(dir: &Path) -> Result<String> {
    let mut sections = vec![format!("Added directory: {}", dir.display())];
    for name in INSTRUCTION_FILES {
        let path = dir.join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        sections.push(format!("## {}\n\n{}", path.display(), content.trim()));
    }
    let skills = SkillRegistry::load_external_dir(dir)?;
    let mut listed: Vec<String> = skills
        .list()
        .into_iter()
        .map(|skill| {
            format!(
                "- {}: {} ({})",
                skill.name,
                skill.description,
                skill.path.display()
            )
        })
        .collect();
    if !listed.is_empty() {
        listed.sort();
        sections.push(format!(
            "## Skills\n\nRead a skill's file when its description matches the task.\n{}",
            listed.join("\n")
        ));
    }
    if sections.len() == 1 {
        sections.push("No AGENTS.md, CLAUDE.md or skills found.".into());
    }
    Ok(sections.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::describe;

    #[test]
    fn collects_instructions_and_skills() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "Use bun.\n").unwrap();
        let skill = dir.path().join(".pi/skills/tdd");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: tdd\ndescription: Test first.\n---\nbody\n",
        )
        .unwrap();

        let text = describe(dir.path()).unwrap();
        assert!(text.contains("AGENTS.md\n\nUse bun."), "{text}");
        assert!(text.contains("- tdd: Test first. ("), "{text}");
        assert!(!text.contains("CLAUDE.md"), "{text}");
    }
}
