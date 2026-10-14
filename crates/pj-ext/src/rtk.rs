//! Port of pi-rtk-optimizer: bash commands go through `rtk rewrite`, and bash
//! output loses ANSI codes and is capped. Keys mirror pi's `config.json`.
//!
//! shortcut: pi's build/test/git/linter output filters are not ported; they only
//! match raw commands, which `rtk rewrite` already turns into compacting `rtk` calls.

use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RtkConfig {
    pub enabled: bool,
    pub mode: RtkMode,
    pub output_compaction: OutputCompaction,
}

impl Default for RtkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: RtkMode::Rewrite,
            output_compaction: OutputCompaction::default(),
        }
    }
}

impl RtkConfig {
    pub fn rewrites(&self) -> bool {
        self.enabled && self.mode == RtkMode::Rewrite
    }
}

/// pi's `suggest` only showed a TUI notice; pj has no such notice, so it does not rewrite.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RtkMode {
    Rewrite,
    Suggest,
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OutputCompaction {
    pub enabled: bool,
    pub strip_ansi: bool,
    pub truncate: Truncate,
}

impl Default for OutputCompaction {
    fn default() -> Self {
        Self {
            enabled: true,
            strip_ansi: true,
            truncate: Truncate::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Truncate {
    pub enabled: bool,
    pub max_chars: usize,
}

impl Default for Truncate {
    fn default() -> Self {
        Self {
            enabled: true,
            max_chars: 12_000,
        }
    }
}

/// Reads the result of `rtk rewrite <command>`: exit 0 or 3 with a different
/// command means rewrite, 1 means no rtk equivalent, 2 means rtk refuses.
pub fn rewritten_command(original: &str, exit_code: Option<i32>, stdout: &str) -> Option<String> {
    if !matches!(exit_code, Some(0 | 3)) {
        return None;
    }
    let rewritten = stdout.trim();
    (!rewritten.is_empty() && rewritten != original).then(|| rewritten.to_string())
}

pub fn is_rtk_command(command: &str) -> bool {
    let trimmed = command.trim_start();
    trimmed == "rtk" || trimmed.starts_with("rtk ")
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").expect("valid regex")
});

/// The bash output the model sees.
pub fn compact_output(output: String, config: &OutputCompaction) -> String {
    if !config.enabled {
        return output;
    }
    let mut output = if config.strip_ansi && output.contains('\x1b') {
        ANSI.replace_all(&output, "").into_owned()
    } else {
        output
    };
    if config.truncate.enabled && output.len() > config.truncate.max_chars {
        let mut end = config.truncate.max_chars;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        output.push_str("\n... (output truncated)");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_needs_success_and_a_different_command() {
        assert_eq!(
            rewritten_command("git status", Some(3), "rtk git status\n").as_deref(),
            Some("rtk git status")
        );
        assert_eq!(rewritten_command("echo hi", Some(1), ""), None);
        assert_eq!(rewritten_command("rm x", Some(2), "rtk rm x"), None);
        assert_eq!(rewritten_command("ls", Some(0), "ls"), None);
    }

    #[test]
    fn strips_ansi_and_caps_on_a_char_boundary() {
        let config = OutputCompaction {
            truncate: Truncate {
                enabled: true,
                max_chars: 5,
            },
            ..OutputCompaction::default()
        };
        assert_eq!(
            compact_output("\x1b[31mabcd\x1b[0m".into(), &config),
            "abcd"
        );
        assert_eq!(
            compact_output("abcdéf".into(), &config),
            "abcd\n... (output truncated)"
        );
    }

    #[test]
    fn reads_pi_config_keys() {
        let config: RtkConfig = toml::from_str(
            "enabled = true\nmode = \"suggest\"\n[outputCompaction.truncate]\nmaxChars = 9\n",
        )
        .unwrap();
        assert!(!config.rewrites());
        assert_eq!(config.output_compaction.truncate.max_chars, 9);
        assert!(config.output_compaction.strip_ansi);
    }
}
