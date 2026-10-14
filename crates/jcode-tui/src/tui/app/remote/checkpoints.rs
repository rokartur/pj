//! pj: git checkpoints for `/rewind` (port of pi's git-checkpoint). The client takes
//! them because it sends the user messages and runs `/rewind`.

use super::*;
use pj_ext::checkpoint;
use std::path::PathBuf;

/// A rewind that left the code newer than the conversation; `/rewind code` restores it.
pub(in crate::tui::app) struct CodeRestore {
    dir: PathBuf,
    commit: String,
    files: Vec<String>,
}

/// Where checkpoints live, or `None` when the code is on another machine (ssh).
fn checkpoint_target(app: &App) -> Option<(PathBuf, String)> {
    if crate::tui::is_ssh_remote() {
        return None;
    }
    let dir = super::super::commands::active_working_dir(app)?;
    Some((dir, app.remote_session_id.clone()?))
}

/// Saves the worktree before the user message just pushed to the transcript runs.
pub(super) async fn save_before_send(app: &App, message: String) {
    let Some((dir, session_id)) = checkpoint_target(app) else {
        return;
    };
    let message_number = super::key_handling::remote_rewindable_messages(app).len();
    let saved = tokio::task::spawn_blocking(move || {
        checkpoint::prune(&dir);
        checkpoint::save(&dir, &session_id, message_number, &message)
    })
    .await;
    if !matches!(saved, Ok(Some(()))) {
        crate::logging::info("[checkpoint] not saved (no git repo or git failed)");
    }
}

/// Before rewinding to message `keep`, finds the checkpoint of the first user message
/// that goes away and offers `/rewind code` when the code changed since.
pub(super) async fn offer_restore(app: &mut App, keep: usize) {
    app.code_restore = None;
    let Some((dir, session_id)) = checkpoint_target(app) else {
        return;
    };
    let removed_user = super::key_handling::remote_rewindable_messages(app)
        .into_iter()
        .enumerate()
        .skip(keep)
        .find(|(_, message)| message.role == "user")
        .map(|(index, message)| (index + 1, message.content.clone()));
    let Some((message_number, message)) = removed_user else {
        return;
    };
    let found = tokio::task::spawn_blocking(move || {
        let then = checkpoint::find(&dir, &session_id, message_number, &message)?;
        let now = checkpoint::snapshot(&dir, "pj rewind")?;
        let files = checkpoint::changed_files(&dir, &then, &now);
        Some(CodeRestore {
            dir,
            commit: then,
            files,
        })
    })
    .await;
    let Ok(Some(restore)) = found else {
        return;
    };
    if restore.files.is_empty() {
        return;
    }
    app.push_display_message(DisplayMessage::system(format!(
        "{} file(s) changed since message {message_number}: {}\n/rewind code restores them (files created since are deleted, ignored files stay).",
        restore.files.len(),
        preview(&restore.files),
    )));
    app.code_restore = Some(restore);
}

/// `/rewind code`: puts the worktree back to the checkpoint found by the last rewind.
pub(super) async fn restore_code(app: &mut App) {
    let Some(restore) = app.code_restore.take() else {
        app.push_display_message(DisplayMessage::error(
            "Nothing to restore: /rewind N first.".to_string(),
        ));
        return;
    };
    let count = restore.files.len();
    let restored = tokio::task::spawn_blocking(move || {
        let Some(undo) = checkpoint::snapshot(&restore.dir, "pj before /rewind code") else {
            anyhow::bail!("could not snapshot the current code, nothing restored");
        };
        checkpoint::restore(&restore.dir, &restore.commit)?;
        Ok(undo)
    })
    .await;
    match restored {
        Ok(Ok(undo)) => app.push_display_message(DisplayMessage::system(format!(
            "Restored {count} file(s). Undo: git restore --source {} --worktree -- :/",
            &undo[..10.min(undo.len())]
        ))),
        Ok(Err(error)) => app.push_display_message(DisplayMessage::error(format!("{error:#}"))),
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!("restore failed: {error}")))
        }
    }
}

fn preview(files: &[String]) -> String {
    let shown = files[..files.len().min(5)].join(", ");
    if files.len() > 5 {
        format!("{shown}, ...")
    } else {
        shown
    }
}
