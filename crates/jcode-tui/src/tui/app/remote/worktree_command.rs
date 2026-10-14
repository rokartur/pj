//! pj `/worktree` (port of pi's worktree extension). The client moves into the
//! checkout and reloads into the same session; on resubscribe the server takes the
//! new working directory (`subscribe_working_dir_replacement`).

use super::*;

pub(super) fn handle(app: &mut App, name: &str) {
    if super::super::commands_dispatch::ssh_local_action_blocked(app, "/worktree") {
        return;
    }
    let Some(dir) = super::super::commands::active_working_dir(app) else {
        app.push_display_message(DisplayMessage::error(
            "No working directory for this session.",
        ));
        return;
    };
    if name.is_empty() {
        show_list(app, &dir);
        return;
    }
    let target = match pj_ext::worktree::open(&dir, name) {
        Ok(target) => target,
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!("/worktree: {error:#}")));
            return;
        }
    };
    if dir.canonicalize().ok() == target.canonicalize().ok() {
        app.push_display_message(DisplayMessage::system(format!(
            "Already in {}.",
            target.display()
        )));
        return;
    }
    if let Err(error) = std::env::set_current_dir(&target) {
        app.push_display_message(DisplayMessage::error(format!(
            "/worktree: cannot enter {}: {error}",
            target.display()
        )));
        return;
    }
    app.push_display_message(DisplayMessage::system(format!(
        "Moving this session to {}. Paths and git state in earlier messages refer to {}.",
        target.display(),
        dir.display()
    )));
    let session_id = app.reload_handoff_session_id();
    app.save_input_for_reload(&session_id);
    app.reload_requested = Some(session_id);
    app.should_quit = true;
}

fn show_list(app: &mut App, dir: &std::path::Path) {
    let worktrees = match pj_ext::worktree::list(dir) {
        Ok(worktrees) => worktrees,
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!("/worktree: {error:#}")));
            return;
        }
    };
    let mut text = String::from("Worktrees:\n");
    for (index, worktree) in worktrees.iter().enumerate() {
        let current = if dir.starts_with(&worktree.path) {
            "*"
        } else {
            " "
        };
        text.push_str(&format!(
            "{current} {} {}  {}\n",
            index + 1,
            worktree.branch,
            worktree.path.display()
        ));
    }
    text.push_str(
        "\n/worktree N|branch|dir moves this session; an unknown branch creates <main>-<branch>.",
    );
    app.push_display_message(DisplayMessage::system(text));
}
