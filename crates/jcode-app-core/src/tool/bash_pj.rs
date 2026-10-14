//! pj token savers on the `bash` tool: pi's bash-guard and rtk rewrite.

use pj_ext::rtk::RtkConfig;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command as TokioCommand;

const RTK_REWRITE_TIMEOUT: Duration = Duration::from_secs(3);

/// Applies pi's bash-guard, then rewrites the command through `rtk rewrite`.
/// A missing or slow `rtk` leaves the command unchanged.
pub(super) async fn prepare_command(command: &str) -> anyhow::Result<Option<String>> {
    if let Some(refusal) = pj_ext::guards::guard_bash(command) {
        anyhow::bail!(refusal);
    }
    let rtk: RtkConfig = crate::config::config().pj_section("rtk");
    // rtk rewrites to bash syntax; on Windows the tool runs cmd.exe.
    if cfg!(windows) || !rtk.rewrites() || pj_ext::rtk::is_rtk_command(command) {
        return Ok(None);
    }
    let rewrite = TokioCommand::new("rtk")
        .arg("rewrite")
        .arg(command)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let Ok(Ok(output)) = tokio::time::timeout(RTK_REWRITE_TIMEOUT, rewrite).await else {
        return Ok(None);
    };
    Ok(pj_ext::rtk::rewritten_command(
        command,
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
    ))
}

pub(super) fn compact_output(output: String) -> String {
    let rtk: RtkConfig = crate::config::config().pj_section("rtk");
    pj_ext::rtk::compact_output(output, &rtk.output_compaction)
}
