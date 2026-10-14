//! Port of pi's `bash-guard`: blocks bash calls that waste tokens or wall time.

use regex::Regex;
use std::sync::LazyLock;

// Audit of 53k pi bash calls: 10k were `cat`/`sed -n`/`head` file reads, and blind
// `sleep N` burned 41h of wall time.
static FILE_READ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?:cd \S+ && )?(?:cat|sed -n ['"]?\d+,\d+p['"]?|head -n? ?\d+) ([^\s|<>;&$`*?]+)$"#,
    )
    .expect("valid regex")
});
static SLEEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[;&|]\s*)sleep\s+(\d+)").expect("valid regex"));
static LOOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:for|while|until)\b").expect("valid regex"));
const MAX_BLIND_SLEEP: u64 = 15;

/// Returns the refusal shown to the model, or `None` when the command may run.
pub fn guard_bash(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if let Some(read) = FILE_READ.captures(trimmed) {
        return Some(format!(
            "Use read({}) with offset/limit instead of bash; edits need anchors from read.",
            &read[1]
        ));
    }
    let blind_sleep = SLEEP
        .captures(trimmed)
        .and_then(|sleep| sleep[1].parse::<u64>().ok())
        .is_some_and(|seconds| seconds >= MAX_BLIND_SLEEP);
    if blind_sleep && !LOOP.is_match(trimmed) {
        return Some(
            "No blind sleep. Server: curl -sf --retry 30 --retry-all-errors --retry-delay 1 URL. \
             CI: gh run watch --exit-status, gh pr checks --watch. Long job: bash with \
             run_in_background, then bg."
                .into(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::guard_bash;

    #[test]
    fn blocks_whole_file_reads() {
        for command in [
            "cat src/main.rs",
            "cd crates && sed -n '1,20p' a.rs",
            "head -n 40 a.rs",
        ] {
            let refusal = guard_bash(command).expect(command);
            assert!(refusal.starts_with("Use read("), "{command}: {refusal}");
        }
    }

    #[test]
    fn blocks_long_sleep_outside_loops() {
        assert!(guard_bash("make && sleep 30 && curl x").is_some());
    }

    #[test]
    fn allows_pipelines_short_sleeps_and_polling_loops() {
        for command in [
            "cat a.log | rg error",
            "head -5 a.csv > b.csv",
            "sleep 2",
            "until curl -sf x; do sleep 20; done",
        ] {
            assert_eq!(guard_bash(command), None, "{command}");
        }
    }
}
