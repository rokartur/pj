//! Port of pi's file-search extension: argv for the `fd` and `rg` tools.
//! Patterns always follow `--`, so model input can never be parsed as a flag.

use serde::Deserialize;
use std::path::Path;

const FD_DEFAULT_LIMIT: u32 = 1000;
const FD_MAX_LIMIT: u32 = 10_000;
const FD_MAX_DEPTH: u32 = 64;
const RG_DEFAULT_LIMIT: u32 = 100;
const RG_MAX_LIMIT: u32 = 1000;
const RG_MAX_CONTEXT: u32 = 20;
// Minified or jsonl lines otherwise fill the whole page, sometimes with 0 lines shown.
const RG_MAX_LINE_BYTES: &str = "500";
pub const MAX_OUTPUT_LINES: usize = 2000;
pub const MAX_OUTPUT_BYTES: usize = 50 * 1024;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FdEntryType {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Default, Deserialize)]
pub struct FdParams {
    pub pattern: Option<String>,
    pub path: Option<String>,
    #[serde(rename = "type")]
    pub entry_type: Option<FdEntryType>,
    pub extension: Option<String>,
    #[serde(default)]
    pub glob: bool,
    #[serde(default)]
    pub hidden: bool,
    pub max_depth: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RgParams {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub file_type: Option<String>,
    pub case_sensitive: Option<bool>,
    #[serde(default)]
    pub fixed_strings: bool,
    #[serde(default)]
    pub hidden: bool,
    pub context: Option<u32>,
    pub limit: Option<u32>,
}

pub fn fd_args(params: &FdParams, home: Option<&Path>) -> Vec<String> {
    let mut args = vec!["--color=never".to_string()];
    if params.hidden {
        args.push("--hidden".into());
    }
    if params.glob {
        args.push("--glob".into());
    }
    if let Some(entry_type) = params.entry_type {
        let flag = match entry_type {
            FdEntryType::File => "f",
            FdEntryType::Directory => "d",
            FdEntryType::Symlink => "l",
        };
        args.extend(["--type".into(), flag.into()]);
    }
    if let Some(extension) = &params.extension {
        args.extend([
            "--extension".into(),
            extension.trim_start_matches('.').into(),
        ]);
    }
    if let Some(depth) = params.max_depth {
        args.extend([
            "--max-depth".into(),
            depth.clamp(1, FD_MAX_DEPTH).to_string(),
        ]);
    }
    let limit = params
        .limit
        .unwrap_or(FD_DEFAULT_LIMIT)
        .clamp(1, FD_MAX_LIMIT);
    args.extend(["--max-results".into(), limit.to_string()]);
    // An empty pattern matches everything, which keeps `path` usable without a pattern.
    args.extend(["--".into(), params.pattern.clone().unwrap_or_default()]);
    args.extend(search_path(params.path.as_deref(), home));
    args
}

pub fn rg_args(params: &RgParams, home: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = [
        "--line-number",
        "--color=never",
        "--no-heading",
        "--with-filename",
        "--max-columns",
        RG_MAX_LINE_BYTES,
        "--max-columns-preview",
    ]
    .map(String::from)
    .into();
    args.push(
        match params.case_sensitive {
            Some(true) => "--case-sensitive",
            Some(false) => "--ignore-case",
            None => "--smart-case",
        }
        .into(),
    );
    if params.fixed_strings {
        args.push("--fixed-strings".into());
    }
    if params.hidden {
        args.push("--hidden".into());
    }
    if let Some(context) = params.context {
        args.extend(["--context".into(), context.min(RG_MAX_CONTEXT).to_string()]);
    }
    if let Some(glob) = &params.glob {
        args.extend(["--glob".into(), glob.clone()]);
    }
    if let Some(file_type) = &params.file_type {
        args.extend(["--type".into(), file_type.clone()]);
    }
    let limit = params
        .limit
        .unwrap_or(RG_DEFAULT_LIMIT)
        .clamp(1, RG_MAX_LIMIT);
    args.extend(["--max-count".into(), limit.to_string()]);
    args.extend(["--".into(), params.pattern.clone()]);
    args.extend(search_path(params.path.as_deref(), home));
    args
}

/// Models sometimes prefix paths with `@` or use `~`; built-in tools accept both.
fn search_path(raw: Option<&str>, home: Option<&Path>) -> Option<String> {
    let path = raw?.trim();
    let path = path.strip_prefix('@').unwrap_or(path);
    let expanded = match (path, home) {
        ("~", Some(home)) => home.display().to_string(),
        (path, Some(home)) if path.starts_with("~/") => home.join(&path[2..]).display().to_string(),
        (path, _) => path.to_string(),
    };
    (!expanded.is_empty()).then_some(expanded)
}

/// Keeps the head of the output within the line and byte limits.
/// Returns the kept text and whether anything was cut.
pub fn cap_output(text: &str) -> (&str, bool) {
    let mut end = 0;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index == MAX_OUTPUT_LINES || end + line.len() > MAX_OUTPUT_BYTES {
            return (text[..end].trim_end_matches('\n'), true);
        }
        end += line.len();
    }
    (text.trim_end_matches('\n'), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_puts_the_pattern_after_the_separator_and_clamps_limits() {
        let params = FdParams {
            pattern: Some("-rf".into()),
            path: Some("@~/src".into()),
            entry_type: Some(FdEntryType::File),
            extension: Some(".rs".into()),
            max_depth: Some(99),
            limit: Some(0),
            ..FdParams::default()
        };
        assert_eq!(
            fd_args(&params, Some(Path::new("/home/a"))),
            [
                "--color=never",
                "--type",
                "f",
                "--extension",
                "rs",
                "--max-depth",
                "64",
                "--max-results",
                "1",
                "--",
                "-rf",
                "/home/a/src",
            ]
        );
    }

    #[test]
    fn rg_defaults_to_smart_case_and_100_matches_per_file() {
        let params = RgParams {
            pattern: "fn main".into(),
            fixed_strings: true,
            context: Some(50),
            ..RgParams::default()
        };
        let args = rg_args(&params, None);
        assert_eq!(
            args[7..],
            [
                "--smart-case",
                "--fixed-strings",
                "--context",
                "20",
                "--max-count",
                "100",
                "--",
                "fn main",
            ]
        );
    }

    #[test]
    fn caps_output_at_whole_lines() {
        let many = "x\n".repeat(MAX_OUTPUT_LINES + 1);
        let (kept, truncated) = cap_output(&many);
        assert!(truncated);
        assert_eq!(kept.lines().count(), MAX_OUTPUT_LINES);
        assert_eq!(cap_output("a\nb\n"), ("a\nb", false));
    }
}
