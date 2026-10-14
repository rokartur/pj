//! pj: condensed tool results (port of pi-condense). The pure parts live in
//! `pj_ext::condense`; this wires them to the agent's turns and provider requests.

use super::*;
use pj_ext::condense::{CondenseConfig, ToolCall};
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

/// Stubs replacing tool outputs in provider requests, keyed by tool_use_id.
#[derive(Default)]
pub(crate) struct CondenseState {
    session_id: String,
    stubs: HashMap<String, String>,
    /// Filled by background summarizer tasks, drained before the next request.
    pub(super) ready: Arc<StdMutex<Vec<(String, String)>>>,
}

fn stubs_path(session_id: &str) -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?
        .join("condense")
        .join(format!("{session_id}.json")))
}

impl Agent {
    /// Swaps condensed tool outputs for their stubs in `messages`.
    pub(super) fn apply_condensed_stubs(&mut self, messages: &mut [Message]) {
        self.sync_condense_state();
        if self.condense.stubs.is_empty() {
            return;
        }
        for message in messages {
            for block in &mut message.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                    && let Some(stub) = self.condense.stubs.get(tool_use_id.as_str())
                {
                    content.clone_from(stub);
                }
            }
        }
    }

    /// Loads the stubs of the current session and takes in finished summaries.
    fn sync_condense_state(&mut self) {
        let session_id = self.session.id.clone();
        if self.condense.session_id != session_id {
            let loaded =
                stubs_path(&session_id).and_then(|path| pj_ext::condense::load_stubs(&path));
            self.condense.stubs = loaded.unwrap_or_else(|error| {
                logging::warn(&format!("[condense] stubs not loaded: {error:#}"));
                HashMap::new()
            });
            self.condense.session_id = session_id;
        }
        let ready: Vec<(String, String)> = {
            let mut ready = self
                .condense
                .ready
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            ready.drain(..).collect()
        };
        if ready.is_empty() {
            return;
        }
        self.condense.stubs.extend(ready);
        let saved = stubs_path(&self.condense.session_id)
            .and_then(|path| pj_ext::condense::save_stubs(&path, &self.condense.stubs));
        if let Err(error) = saved {
            logging::warn(&format!("[condense] stubs not saved: {error:#}"));
        }
    }

    /// Summarizes the tool results added since `start_message_index` in the
    /// background. A failed summary leaves the raw outputs in place.
    pub(super) fn condense_turn(&mut self, start_message_index: usize) {
        let config: CondenseConfig = crate::config::config().pj_section("condense");
        if !config.enabled {
            return;
        }
        let calls = config.select(self.tool_calls_since(start_message_index));
        if calls.is_empty() {
            return;
        }
        let Some(sidecar) = summarizer(&config.summarizer_model) else {
            logging::warn(&format!(
                "[condense] unsupported summarizerModel {:?}; use anthropic/<model> or openai/<model>",
                config.summarizer_model
            ));
            return;
        };
        let ready = Arc::clone(&self.condense.ready);
        tokio::spawn(async move {
            let prompt = pj_ext::condense::summarizer_prompt(&calls);
            match sidecar
                .complete(pj_ext::condense::SUMMARIZER_SYSTEM, &prompt)
                .await
            {
                Ok(summary) if !summary.trim().is_empty() => {
                    let stubs = pj_ext::condense::stubs(&calls, &summary);
                    ready
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .extend(stubs);
                }
                Ok(_) => logging::warn("[condense] summarizer returned nothing; outputs kept"),
                Err(error) => logging::warn(&format!(
                    "[condense] summary failed, outputs kept: {error:#}"
                )),
            }
        });
    }

    fn tool_calls_since(&self, start_message_index: usize) -> Vec<ToolCall> {
        let mut uses: HashMap<&str, (&str, &serde_json::Value)> = HashMap::new();
        let mut calls = Vec::new();
        for stored in self.session.messages.iter().skip(start_message_index) {
            for block in &stored.content {
                match block {
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    } => {
                        uses.insert(id, (name, input));
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        let Some((name, input)) = uses.get(tool_use_id.as_str()) else {
                            continue;
                        };
                        calls.push(ToolCall {
                            id: tool_use_id.clone(),
                            name: name.to_string(),
                            input: (*input).clone(),
                            output: content.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }
        calls
    }
}

fn summarizer(model: &str) -> Option<crate::sidecar::Sidecar> {
    if let Some(model) = model.strip_prefix("anthropic/") {
        return Some(crate::sidecar::Sidecar::with_claude_model(model));
    }
    let model = model.strip_prefix("openai/")?;
    Some(crate::sidecar::Sidecar::with_openai_model(model, None))
}
