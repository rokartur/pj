//! `context_tree_query`: returns the original output of condensed tool results.

use super::{Tool, ToolContext, ToolOutput};
use crate::message::ContentBlock;
use crate::session::Session;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub struct ContextTreeQueryTool;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    tool_call_ids: Vec<String>,
}

#[async_trait]
impl Tool for ContextTreeQueryTool {
    fn name(&self) -> &str {
        "context_tree_query"
    }

    fn description(&self) -> &str {
        "Return the original output of condensed tool results by their refs."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["toolCallIds"],
            "properties": {
                "toolCallIds": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Refs named in a condensed tool result."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: Input = serde_json::from_value(input)?;
        let session = Session::load(&ctx.session_id)?;
        let mut sections = Vec::with_capacity(input.tool_call_ids.len());
        for id in &input.tool_call_ids {
            let original = session
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .find_map(|block| match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } if tool_use_id == id => Some(content.as_str()),
                    _ => None,
                });
            sections.push(match original {
                Some(content) => format!("[{id}]\n{content}"),
                None => format!("[{id}] not found in this session"),
            });
        }
        Ok(ToolOutput::new(sections.join("\n\n")))
    }
}
