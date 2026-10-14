//! `ask_user_question` (port of rpiv-ask-user-question). Each question travels to
//! the client over the stdin request channel and the user's line comes back.

use super::{StdinInputRequest, Tool, ToolContext, ToolOutput};
use anyhow::{Context as _, Result};
use async_trait::async_trait;
use pj_ext::ask::{Questions, REQUEST_PREFIX, summary};
use serde_json::{Value, json};

pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "ask_user_question"
    }

    fn description(&self) -> &str {
        "Ask the user 1-4 multiple-choice questions and wait for the answers."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["questions"],
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "required": ["question", "header", "options"],
                        "properties": {
                            "question": {"type": "string", "description": "One question, ending with ?"},
                            "header": {"type": "string", "description": "Short label, 16 chars or fewer."},
                            "multiSelect": {"type": "boolean", "description": "Allow several options."},
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "required": ["label", "description"],
                                    "properties": {
                                        "label": {"type": "string", "description": "1-5 words; mark a recommended one."},
                                        "description": {"type": "string", "description": "What choosing it means."}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let questions: Questions = serde_json::from_value(input)?;
        questions.validate()?;
        let stdin = ctx.stdin_request_tx.as_ref().context(
            "no interactive user in this session; decide yourself and say what you assumed",
        )?;
        let mut answers = Vec::with_capacity(questions.questions.len());
        for (index, question) in questions.questions.iter().enumerate() {
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            stdin
                .send(StdinInputRequest {
                    request_id: format!("{REQUEST_PREFIX}{}:{index}", ctx.tool_call_id),
                    prompt: question.prompt(),
                    is_password: false,
                    response_tx,
                })
                .ok()
                .context("the client went away before answering")?;
            let reply = response_rx
                .await
                .context("the question was dismissed without an answer")?;
            answers.push((question.question.as_str(), question.answer(&reply)));
        }
        Ok(ToolOutput::new(summary(&answers)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sends_each_question_and_maps_the_reply() {
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<StdinInputRequest>();
        tokio::spawn(async move {
            while let Some(request) = stdin_rx.recv().await {
                assert!(request.request_id.starts_with(REQUEST_PREFIX));
                assert!(request.prompt.contains("1. Bun"), "{}", request.prompt);
                let _ = request.response_tx.send("2".to_string());
            }
        });
        let ctx = ToolContext {
            session_id: "s".into(),
            message_id: "m".into(),
            tool_call_id: "call".into(),
            working_dir: None,
            stdin_request_tx: Some(stdin_tx),
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };
        let input = json!({"questions": [{
            "question": "Which runtime?",
            "header": "Runtime",
            "options": [
                {"label": "Bun", "description": "fast"},
                {"label": "Node", "description": "everywhere"}
            ]
        }]});
        let output = AskUserQuestionTool.execute(input, ctx).await.unwrap();
        assert_eq!(
            output.output,
            "User has answered your questions: \"Which runtime?\"=\"Node\". You can now continue with the user's answers in mind."
        );
    }
}
