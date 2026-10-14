//! `pj import-pi`: one-shot conversion of a pi agent directory into `~/.pj`.
//!
//! The jcode config is the base and pi values override it. pi-only settings land
//! under `[pj.*]` with pi's own (camelCase) keys so pj-ext modules can read them
//! with the same names the pi extensions used. Nothing under the pi or jcode
//! directories is modified.

use anyhow::{Context, Result};
use serde_json::Value as Json;
use std::path::{Path, PathBuf};
use toml::{Table, Value};

pub struct ImportPaths {
    pub pi_dir: PathBuf,
    pub jcode_dir: PathBuf,
    pub pj_dir: PathBuf,
}

pub struct ImportReport {
    pub config: PathBuf,
    pub backup: Option<PathBuf>,
    pub copied: Vec<PathBuf>,
}

/// pi `settings.json` keys that describe the look and feel of the TUI.
const UI_KEYS: &[&str] = &[
    "theme",
    "tuiMode",
    "outputPad",
    "editorPaddingX",
    "hideThinkingBlock",
    "markdown",
    "fullscreenScrollbar",
    "fullscreenCopyOnSelect",
    "fullscreenWheelScrollLines",
    "fullscreenExitOutput",
    "doubleEscapeAction",
    "steeringMode",
    "followUpMode",
    "autocompleteMaxVisible",
    "quietStartup",
    "showHardwareCursor",
    "collapseChangelog",
    "treeFilterMode",
];

pub fn run(paths: &ImportPaths) -> Result<ImportReport> {
    let base = read_base_config(&paths.jcode_dir.join("config.toml"))?;
    let config = build_config(base, &paths.pi_dir)?;

    std::fs::create_dir_all(&paths.pj_dir)
        .with_context(|| format!("create {}", paths.pj_dir.display()))?;
    let config_path = paths.pj_dir.join("config.toml");
    let backup = if config_path.exists() {
        let backup = paths.pj_dir.join("config.toml.bak");
        std::fs::copy(&config_path, &backup)
            .with_context(|| format!("back up {}", config_path.display()))?;
        Some(backup)
    } else {
        None
    };
    std::fs::write(&config_path, toml::to_string_pretty(&config)?)
        .with_context(|| format!("write {}", config_path.display()))?;

    let copied = copy_side_files(paths)?;
    Ok(ImportReport {
        config: config_path,
        backup,
        copied,
    })
}

pub fn build_config(base: Table, pi_dir: &Path) -> Result<Table> {
    let settings = read_json(&pi_dir.join("settings.json"))?
        .with_context(|| format!("{} has no settings.json", pi_dir.display()))?;
    let mut config = base;

    let provider = subtable(&mut config, "provider");
    if let Some(name) = settings["defaultProvider"].as_str() {
        provider.insert("default_provider".into(), jcode_provider(name).into());
    }
    if let Some(model) = settings["defaultModel"].as_str() {
        provider.insert("default_model".into(), model.into());
    }
    if let Some(level) = settings["defaultThinkingLevel"].as_str() {
        provider.insert(
            "anthropic_reasoning_effort".into(),
            anthropic_effort(level).into(),
        );
    }
    if let Some(hide) = settings["hideThinkingBlock"].as_bool() {
        subtable(&mut config, "display").insert("show_thinking".into(), (!hide).into());
    }

    let mut pj = Table::new();
    let ui: Table = UI_KEYS
        .iter()
        .filter_map(|key| Some((key.to_string(), to_toml(settings.get(*key)?)?)))
        .collect();
    pj.insert("ui".into(), ui.into());
    insert_json(&mut pj, "compaction", settings.get("compaction"));
    // budgetGuard is not imported: jcode compacts natively at 80% of the context window.
    insert_json(&mut pj, "condense", settings.get("contextPrune"));
    // cacheWarming is not imported: pi-cache-optimizer's README notes warming adds
    // nothing with 1h cache retention, which jcode uses by default (anthropic_cache_ttl_1h).
    pj.insert("skills".into(), skills_table(&settings).into());
    pj.insert("ext".into(), ext_table(&settings).into());

    let models = read_json(&pi_dir.join("models.json"))?;
    let (model_table, provider_table) = models_tables(models.as_ref(), &settings);
    pj.insert("models".into(), model_table.into());
    if !provider_table.is_empty() {
        pj.insert("providers".into(), provider_table.into());
    }

    let side_configs = [
        ("plan_mode", "pi-plan-mode.json"),
        ("sense", "pi-sense.json"),
        ("goal", "pi-goal-x-settings.json"),
        ("web_search", "web-search.json"),
        ("rtk", "extensions/pi-rtk-optimizer/config.json"),
        ("trust", "trust.json"),
    ];
    for (section, file) in side_configs {
        insert_json(&mut pj, section, read_json(&pi_dir.join(file))?.as_ref());
    }

    let mut subagents = Table::new();
    for source in [
        read_json(&pi_dir.join("subagents.json"))?,
        settings.get("subagents").cloned(),
        read_json(&pi_dir.join("extensions/subagent/config.json"))?,
    ] {
        if let Some(Value::Table(table)) = source.as_ref().and_then(to_toml) {
            subagents.extend(table);
        }
    }
    pj.insert("subagents".into(), subagents.into());

    config.insert("pj".into(), pj.into());
    Ok(config)
}

fn read_base_config(path: &Path) -> Result<Table> {
    if !path.exists() {
        return Ok(Table::new());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    text.parse::<Table>()
        .with_context(|| format!("parse {}", path.display()))
}

fn read_json(path: &Path) -> Result<Option<Json>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let json = serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(json))
}

fn subtable<'a>(table: &'a mut Table, key: &str) -> &'a mut Table {
    let entry = table
        .entry(key)
        .or_insert_with(|| Value::Table(Table::new()));
    if !entry.is_table() {
        *entry = Value::Table(Table::new());
    }
    entry.as_table_mut().expect("entry was just made a table")
}

fn insert_json(table: &mut Table, key: &str, json: Option<&Json>) {
    if let Some(value) = json.and_then(to_toml) {
        table.insert(key.into(), value);
    }
}

/// TOML has no null, so null fields are dropped instead of guessed.
fn to_toml(json: &Json) -> Option<Value> {
    match json {
        Json::Null => None,
        Json::Bool(value) => Some(Value::Boolean(*value)),
        Json::Number(number) => number
            .as_i64()
            .map(Value::Integer)
            .or_else(|| number.as_f64().map(Value::Float)),
        Json::String(value) => Some(Value::String(value.clone())),
        Json::Array(items) => Some(Value::Array(items.iter().filter_map(to_toml).collect())),
        Json::Object(map) => Some(Value::Table(
            map.iter()
                .filter_map(|(key, value)| Some((key.clone(), to_toml(value)?)))
                .collect(),
        )),
    }
}

fn jcode_provider(pi_provider: &str) -> &str {
    match pi_provider {
        "anthropic" => "claude",
        "openai-codex" => "openai",
        other => other,
    }
}

/// pi thinking levels are off|minimal|low|medium|high|xhigh; Anthropic effort has no off or minimal.
fn anthropic_effort(pi_level: &str) -> &str {
    match pi_level {
        "off" => "none",
        "minimal" => "low",
        other => other,
    }
}

/// `"-skills/<name>/SKILL.md"` disables a skill; other entries keep the default (enabled).
fn skills_table(settings: &Json) -> Table {
    let mut disabled = Vec::new();
    for entry in settings["skills"].as_array().into_iter().flatten() {
        let Some(path) = entry.as_str().and_then(|entry| entry.strip_prefix('-')) else {
            continue;
        };
        let name = Path::new(path)
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str());
        if let Some(name) = name {
            disabled.push(Value::from(name));
        }
    }
    let mut table = Table::new();
    table.insert("disabled".into(), Value::Array(disabled));
    table
}

/// Every extension pi lists is loaded through its bundles, so all of them count as enabled.
fn ext_table(settings: &Json) -> Table {
    let mut enabled = Vec::new();
    for package in settings["packages"].as_array().into_iter().flatten() {
        let Some(source) = package["source"].as_str() else {
            continue;
        };
        let Some(npm) = source.strip_prefix("npm:") else {
            continue;
        };
        let name = npm.rsplit('/').next().unwrap_or(npm);
        enabled.push(Value::from(name));
    }
    for entry in settings["extensions"].as_array().into_iter().flatten() {
        let Some(path) = entry.as_str() else { continue };
        let path = path.trim_start_matches('!');
        let path = path.strip_prefix("extensions/").unwrap_or(path);
        let name = path
            .strip_suffix("/index.ts")
            .or_else(|| path.strip_suffix(".ts"))
            .unwrap_or(path);
        enabled.push(Value::from(name));
    }
    let mut table = Table::new();
    table.insert("enabled".into(), Value::Array(enabled));
    table
}

/// Returns `[pj.models]` keyed `"<provider>/<id>"` and `[pj.providers]` holding provider compat flags.
fn models_tables(models: Option<&Json>, settings: &Json) -> (Table, Table) {
    let mut model_table = Table::new();
    let mut provider_table = Table::new();
    let providers = models
        .and_then(|models| models["providers"].as_object())
        .into_iter()
        .flatten();
    for (provider, config) in providers {
        if let Some(compat) = config.get("compat").and_then(to_toml) {
            let mut table = Table::new();
            table.insert("compat".into(), compat);
            provider_table.insert(provider.clone(), table.into());
        }
        let overrides = config["modelOverrides"].as_object().into_iter().flatten();
        for (id, model) in overrides {
            if let Some(value) = to_toml(model) {
                model_table.insert(format!("{provider}/{id}"), value);
            }
        }
    }
    let levels = settings["modelThinkingLevels"]
        .as_object()
        .into_iter()
        .flatten();
    for (model, level) in levels {
        let Some(level) = level.as_str() else {
            continue;
        };
        subtable(&mut model_table, model).insert("thinking".into(), level.into());
    }
    (model_table, provider_table)
}

fn copy_side_files(paths: &ImportPaths) -> Result<Vec<PathBuf>> {
    let files = [
        (paths.pi_dir.join("SYSTEM.md"), "system-prompt.md"),
        (
            paths.jcode_dir.join("prompt-overlay.md"),
            "prompt-overlay.md",
        ),
        (
            paths.pi_dir.join("claude-accounts.json"),
            "claude-accounts.json",
        ),
    ];
    let mut copied = Vec::new();
    for (source, name) in files {
        if !source.is_file() {
            continue;
        }
        let target = paths.pj_dir.join(name);
        std::fs::copy(&source, &target)
            .with_context(|| format!("copy {} to {}", source.display(), target.display()))?;
        copied.push(target);
    }
    // claude-accounts.json holds OAuth credentials.
    restrict_to_owner(&paths.pj_dir.join("claude-accounts.json"))?;

    for dir in ["prompts", "review-axes"] {
        let source = paths.pi_dir.join(dir);
        if source.is_dir() {
            let target = paths.pj_dir.join(dir);
            copy_dir(&source, &target)?;
            copied.push(target);
        }
    }
    Ok(copied)
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) -> Result<()> {
    Ok(())
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target).with_context(|| format!("create {}", target.display()))?;
    for entry in std::fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let path = entry?.path();
        let destination = target.join(path.file_name().expect("read_dir entries have names"));
        if path.is_dir() {
            copy_dir(&path, &destination)?;
        } else {
            std::fs::copy(&path, &destination)
                .with_context(|| format!("copy {}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn pi_fixture() -> tempfile::TempDir {
        let pi = tempfile::tempdir().unwrap();
        write(
            pi.path(),
            "settings.json",
            r#"{
              "theme": "fx",
              "defaultProvider": "anthropic",
              "defaultModel": "claude-opus-5-5",
              "defaultThinkingLevel": "minimal",
              "hideThinkingBlock": true,
              "outputPad": 1,
              "packages": [
                {"source": "npm:@narumitw/pi-plan-mode", "extensions": ["!**"]},
                {"source": "npm:pi-condense"},
                "./bundles"
              ],
              "extensions": ["!extensions/bash-guard.ts", "!extensions/status-footer/index.ts"],
              "skills": ["-skills/remotion-render/SKILL.md", "+skills/kept/SKILL.md"],
              "compaction": {"reserveTokens": 100000},
              "contextPrune": {"enabled": true, "autoBudgetThreshold": null},
              "subagents": {"agentOverrides": {"oracle": {"systemPromptMode": "append"}}},
              "modelThinkingLevels": {"anthropic/claude-opus-5": "high"},
              "cacheWarming": "idle"
            }"#,
        );
        write(
            pi.path(),
            "models.json",
            r#"{"providers": {"anthropic": {
                "compat": {"supportsLongCacheRetention": false},
                "modelOverrides": {"claude-opus-5-5": {"contextWindow": 500000}}
            }}}"#,
        );
        write(pi.path(), "trust.json", r#"{"/Users/artur": true}"#);
        write(
            pi.path(),
            "subagents.json",
            r#"{"toolDescriptionMode": "compact"}"#,
        );
        pi
    }

    #[test]
    fn maps_pi_settings_over_jcode_base() {
        let pi = pi_fixture();
        let base: Table = r#"
            [provider]
            default_model = "gpt-5"
            openai_reasoning_effort = "low"
            [display]
            show_thinking = true
        "#
        .parse()
        .unwrap();

        let config = build_config(base, pi.path()).unwrap();
        let expected: Table = r#"
            [provider]
            default_model = "claude-opus-5-5"
            default_provider = "claude"
            openai_reasoning_effort = "low"
            anthropic_reasoning_effort = "low"
            [display]
            show_thinking = false
            [pj.ui]
            theme = "fx"
            outputPad = 1
            hideThinkingBlock = true
            [pj.compaction]
            reserveTokens = 100000
            [pj.condense]
            enabled = true
            [pj.skills]
            disabled = ["remotion-render"]
            [pj.ext]
            enabled = ["pi-plan-mode", "pi-condense", "bash-guard", "status-footer"]
            [pj.models."anthropic/claude-opus-5-5"]
            contextWindow = 500000
            [pj.models."anthropic/claude-opus-5"]
            thinking = "high"
            [pj.providers.anthropic.compat]
            supportsLongCacheRetention = false
            [pj.trust]
            "/Users/artur" = true
            [pj.subagents]
            toolDescriptionMode = "compact"
            [pj.subagents.agentOverrides.oracle]
            systemPromptMode = "append"
        "#
        .parse()
        .unwrap();
        assert_eq!(config, expected);
    }

    #[test]
    fn second_run_backs_up_previous_config() {
        let pi = pi_fixture();
        write(pi.path(), "SYSTEM.md", "system");
        write(pi.path(), "claude-accounts.json", "{}");
        write(pi.path(), "prompts/review.md", "review");
        let jcode = tempfile::tempdir().unwrap();
        write(jcode.path(), "config.toml", "[provider]\nmax_retries = 3\n");
        let pj = tempfile::tempdir().unwrap();
        let paths = ImportPaths {
            pi_dir: pi.path().into(),
            jcode_dir: jcode.path().into(),
            pj_dir: pj.path().join("home"),
        };

        let first = run(&paths).unwrap();
        assert!(first.backup.is_none());
        std::fs::write(&first.config, "edited = true\n").unwrap();
        let second = run(&paths).unwrap();

        let backup = second.backup.expect("second run writes a backup");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "edited = true\n");
        let written: Table = std::fs::read_to_string(&second.config)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(written["provider"]["max_retries"].as_integer(), Some(3));
        let home = pj.path().join("home");
        assert_eq!(
            std::fs::read_to_string(home.join("system-prompt.md")).unwrap(),
            "system"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("prompts/review.md")).unwrap(),
            "review"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.join("claude-accounts.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
