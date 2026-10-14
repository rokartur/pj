//! `fd` and `rg` tools, ported from pi's file-search extension.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context as _, Result};
use async_trait::async_trait;
use pj_ext::file_search::{FdParams, RgParams, cap_output, fd_args, rg_args};
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command;

pub struct FdTool;
pub struct RgTool;

#[async_trait]
impl Tool for FdTool {
    fn name(&self) -> &str {
        "fd"
    }

    fn description(&self) -> &str {
        "Find files by name with fd (gitignore-aware). Use instead of find or ls -R."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regex matched against file names (or a glob when glob is true). Omit to list everything under path."},
                "path": {"type": "string", "description": "Directory to search. Defaults to the current working directory."},
                "type": {"type": "string", "enum": ["file", "directory", "symlink"], "description": "Only return entries of this type."},
                "extension": {"type": "string", "description": "Only return files with this extension, e.g. 'ts' or 'md'."},
                "glob": {"type": "boolean", "description": "Treat pattern as a glob (e.g. '*.test.ts') instead of a regex."},
                "hidden": {"type": "boolean", "description": "Include hidden files and directories. Defaults to false."},
                "max_depth": {"type": "integer", "minimum": 1, "maximum": 64, "description": "Maximum directory depth to descend (1-64)."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 10000, "description": "Maximum number of results (1-10000). Defaults to 1000."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: FdParams = serde_json::from_value(input)?;
        let args = fd_args(&params, dirs::home_dir().as_deref());
        run("fd", &args, &ctx, "No files found.").await
    }
}

#[async_trait]
impl Tool for RgTool {
    fn name(&self) -> &str {
        "rg"
    }

    fn description(&self) -> &str {
        "Search file contents with ripgrep (smart-case, gitignore-aware)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {"type": "string", "description": "Regex to search for (literal text when fixed_strings is true)."},
                "path": {"type": "string", "description": "File or directory to search. Defaults to the current working directory."},
                "glob": {"type": "string", "description": "Only search files matching this glob, e.g. '*.ts' or 'src/**'."},
                "file_type": {"type": "string", "description": "Only search files of this ripgrep type, e.g. 'ts', 'js', 'py', 'rust'."},
                "case_sensitive": {"type": "boolean", "description": "true forces case-sensitive matching, false forces case-insensitive. Defaults to smart-case."},
                "fixed_strings": {"type": "boolean", "description": "Treat pattern as a literal string instead of a regex."},
                "hidden": {"type": "boolean", "description": "Search hidden files and directories. Defaults to false."},
                "context": {"type": "integer", "minimum": 0, "maximum": 20, "description": "Lines of context to show around each match (0-20)."},
                "limit": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum matches per file (1-1000). Defaults to 100."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: RgParams = serde_json::from_value(input)?;
        let args = rg_args(&params, dirs::home_dir().as_deref());
        run("rg", &args, &ctx, "No matches found.").await
    }
}

async fn run(program: &str, args: &[String], ctx: &ToolContext, empty: &str) -> Result<ToolOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = &ctx.working_dir {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .await
        .with_context(|| format!("{program} is not installed or not on PATH"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // rg exits 1 when nothing matched; everything else non-zero is an error.
    let no_match = program == "rg" && output.status.code() == Some(1);
    if !output.status.success() && !no_match {
        anyhow::bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if stdout.trim().is_empty() {
        return Ok(ToolOutput::new(empty));
    }
    let (kept, truncated) = cap_output(&stdout);
    if !truncated {
        return Ok(ToolOutput::new(kept));
    }
    let full = std::env::temp_dir().join(format!("pj-{program}-{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&full, stdout.as_bytes())
        .with_context(|| format!("save full output to {}", full.display()))?;
    let shown = kept.lines().count();
    Ok(ToolOutput::new(format!(
        "{kept}\n\n[Output truncated: {shown} of {} lines. Full output saved to: {}. Read it with start_line={} to continue.]",
        stdout.lines().count(),
        full.display(),
        shown + 1
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> ToolContext {
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

    #[tokio::test]
    async fn rg_and_fd_search_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();

        let found = RgTool
            .execute(json!({"pattern": "fn main"}), ctx(dir.path()))
            .await
            .unwrap();
        assert_eq!(found.output, "a.rs:1:fn main() {}");
        let none = RgTool
            .execute(json!({"pattern": "absent"}), ctx(dir.path()))
            .await
            .unwrap();
        assert_eq!(none.output, "No matches found.");
        let files = FdTool
            .execute(json!({"extension": "rs"}), ctx(dir.path()))
            .await
            .unwrap();
        assert_eq!(files.output, "a.rs");
    }
}
