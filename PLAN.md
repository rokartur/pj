# M5: port the remaining pi extensions

## Summary
M5 ports only the pi extensions that have no counterpart in jcode. The new behaviors:
- after each finished run: edit hooks plus a project typecheck, with automatic fix rounds;
- the tab title gets a session name from a cheap model;
- `/window`;
- turning skills on and off, plus a skill usage log;
- usagefleet (group budget in the footer, prompt blocking);
- `/cloud`;
- a "needs you" notification when a question is waiting.

Pure logic goes in `crates/pj-ext/src/<name>.rs`, one module per extension, following the existing `condense`/`worktree` pattern. Only the wiring goes into `jcode-*`.

### Not ported, because jcode already has a counterpart
| pi | in pj/jcode |
|---|---|
| background-terminals, subagent | `bg` tool, subagent/swarm |
| voice-whisper-cpp | `/voice` |
| claude-accounts, usage | `/account(s)`, `/usage` and the usage widget |
| budget-guard | built-in compaction |
| goal-dashboard, changes | goal widget, Changes widget and diff |
| turn-clock | turn duration and cache TTL in the status line |
| ghostty-notify | turn-end notifications (M5 adds only "needs you") |
| abort-pending, pending-input-echo | "Interrupting…", queue display |
| error-summary, model-debugger, tool-loader | jcode error formatting, logs, deferred tools |

### Dropped (pi-only patches or pi infrastructure)
transcript-padding, code-block-copy, minimal-user-messages, system-notice, pruner-notices, spinner-final-guard, console-guard, bundle-errors, resume-gap, compact-conflicts, tsc-rs-update (pi keeps updating it), usagefleet-guard (replaced by the usagefleet port), clef-router (your call).

## Changes

### M5a: checks after a run (edit-hooks + typecheck)
- **`pj-ext/src/edit_hooks.rs`**, a 1:1 port of `edit-hooks.ts`:
  - Config is `.agents/edit-hooks.json` with `{"onStop": {"<glob>": "cmd {files}" | ["cmd {file}", "cmd"]}}`.
  - The config used is the nearest one walking up from the file, but never above the project root (the git root).
  - A glob containing `/` matches the path relative to the config directory. Otherwise it matches the file name. Each file goes to the first matching glob.
  - `{files}` gets all the files, `{file}` runs the command once per file, and arguments are shell-quoted.
  - `run_command` runs `/bin/sh -c` in its own process group, with a 120 s timeout (then kills the whole group), output capped at 8000 characters, and an outcome of `exit code N` / `timed out after` / `killed by`.
  - PATH is extended with any of `~/.cargo/bin`, `~/.bun/bin`, `~/go/bin`, `/opt/homebrew/bin` that exist. This fixes the exit 127 we hit with the pi hook.
  - The report text sent to the model is copied verbatim from pi.
- **`pj-ext/src/typecheck.rs`**, a port of `diagnostics/typecheck.ts` without LSP:
  - Detecting which check covers a file:
    - TS: a `typecheck|type-check|check-types|tsc` script in `package.json` runs as `bun run <script>`. Otherwise `tsconfig.json` runs through `tsc-rs`, with `bunx tsc` as the fallback, using the same flags. A solution tsconfig with references checks each reference separately.
    - `go.mod` runs `go vet ./...`.
    - `Cargo.toml` runs `cargo check --message-format short`.
    - `.xcodeproj`/`.xcworkspace` runs `xc build` (from PATH, otherwise `~/.pi/agent/bin/xc`). `Package.swift` runs `swift build`.
    - Files that no check covers are skipped.
  - `parse_problems` ports the `PROBLEM` regex: it skips warnings and notes, attaches up to 6 indented continuation lines, and removes swift's duplicate reports.
  - `decide_gate` keeps a per-check baseline across the session, reports only new errors (at most 30 listed), and uses pi's texts verbatim ("Typecheck of the projects you changed found N new errors. …").
- **Wiring in `jcode-app-core/src/agent/settle.rs`**, called from `run_once` and `run_once_streaming_mpsc_with_display_role` after a successful turn, before `condense_turn`:
  - Edited files are collected from the turn's successful edit tool calls. This reuses `touched_paths` from `tool/mod.rs`, made a crate-visible function. They go into `Agent.settle.pending_files`.
  - An aborted or failed turn keeps its files for the next completed one.
  - On completion, edit hooks run first, then the typecheck, inside `spawn_blocking`. The results go into one message, saved as `Role::User` with `display_role: System` and shown to clients through `ServerEvent::SoftInterruptInjected`.
  - The loop then continues the turn with `run_turn_streaming_mpsc` / `run_turn`. One fix-round counter allows at most 3 per user prompt. After the limit, the message says it stopped retrying and the turn ends.
  - The typecheck baseline lives on the Agent for the life of the session.
  - There is no trust gate (your call).

### M5b: tab title from a model (title-renamer)
- `pj-ext/src/title.rs` provides `clean`, `truncate` (on chars) and `with_project(topic, dir)`, which builds `"<dir>｜<topic>"`. The limit is 48 characters, matching `tab_title`. It also holds the prompt copied from pi ("Write one terminal tab title… Format: <type>: <area> <what>…").
- The TUI acts after the first finished turn (`Done`) when the session has no `custom_title` and has not tried yet:
  - A tokio task runs the sidecar for `[pj.title] model`, default `anthropic/claude-haiku-4-5`, with a 10 s timeout. If it fails, it uses the first line of the first message.
  - The result arrives on a channel, and `handle_tick` sends `remote.rename_session(Some(title))`, but only if `custom_title` is still empty.
  - The tab then reads `pj｜fix: footer… | pj` through the existing `tab_title`.
  - `[pj.title] enabled = true` by default.
- `summarizer(model)` moves from `agent/condense.rs` to `Sidecar::for_model_spec(&str) -> Option<Sidecar>` in `jcode-base`, because it now has two callers.

### M5c: `/window [200k|500k|1m|reset]`
- New request `Request::SetContextWindow { id, tokens: Option<u64> }`, with tag `set_context_window`.
- The Agent gets `context_window_override: Option<usize>`.
  - `provider_context_window()` returns the override, falling back to the provider value.
  - Every place that takes the budget from `self.provider.context_window()` switches to `provider_context_window()`: compaction, `set_budget`, `ModelChanged`/resume `context_window`.
  - Switching model or route clears the override.
- The server rejects values above the current route's window: "this route serves at most N; use the [1m] model alias". `reset` restores the default.
- On `Done`, the client calls `set_context_limit_and_sync_budget`, so the footer `ctx %` updates. With no argument it prints the current window and usage.
- API requests themselves do not change. This only moves the compaction budget, like pi.

### M5d: skills
- `[pj.skills] disabled = [...]` (already written by `import-pi`) now takes effect. Disabled skills drop out of the prompt list, slash invocation, and `skill` tool loading, which returns "disabled; /skills on <name>".
- The filter reads `config().pj_section("skills")` each time it is used, so the client and the server see a change right away (the config reloads on fingerprint).
- `/skills on|off <name>` loads the config, edits `pj.skills.disabled`, and saves (the `commands_colors::persist` pattern). The `/skills` report marks disabled skills as `off`. This is a shortcut: there is no interactive search list like pi's.
- `/skill-usage` uses the log `~/.cache/pi-skill-usage.jsonl`, shared with pi, one JSON line per use `{at, skill, by, cwd}`:
  - `by: agent` for loading a skill through the `skill` tool and for a `read` of a registered SKILL.md (compared by realpath);
  - `by: user` for `/<skill>`.
  - The command prints counts for the loaded skills, least used first, in pi's format.

### M5e: usagefleet
- `pj-ext/src/usagefleet.rs` holds pure functions from pi: `format_limits` (stale after 5 min), `format_group_limits` (null when it repeats the account numbers), `detect_account_uuid`, `resolve_token` (`USAGEFLEET_TOKEN`, then `accountTokens[uuid]`, then `token`, then the first token found), and `block_message`.
- In the TUI:
  - **Refresh:** runs in the background via `shared_http_client` (`GET https://usagefleet.com/api/v1/limits`, header `x-api-key`, 5 s timeout) at startup, after each `Done`, and on each submit.
  - **Inputs:** config from `$USAGEFLEET_CONFIG` or `~/.config/usagefleet/config.json`. The account uuid comes from `~/.config/usagefleet/pi-claude/.claude.json`, otherwise from `$CLAUDE_CONFIG_DIR`/`~/.claude.json`. Without a config file, the feature is off.
  - **Footer:** `Artur: 5h 30% · 7d 17%` joins the limits section. It drops out first on narrow terminals and hides along with the limits when the floating overview is visible.
  - **Blocking:** a submit is refused with the `block_message` text when the latest result says `blocked: true`. Errors, offline, and a missing token let the prompt through (fail open).

### M5f: `/cloud` and "needs you"
- `/cloud <task>` opens a Ghostty tab running `claude --cloud "$PJ_CLOUD_TASK"`, using pi's AppleScript on macOS and `ghostty --working-directory=… -e` on Linux. The task is passed in an env var, and errors show in the status line.
- When an `ask_user_question` request arrives and the terminal is not focused, a desktop notification is sent: `pj: needs you · <dir>` with the first question (up to 200 characters). It follows `notifications.turn_complete`/focus, like turn notifications.

## Tests (one per behavior)
- **edit_hooks:**
  - config errors (`onStop` is not a map, a value is not a string or list);
  - `{files}` / `{file}` / plain command;
  - glob with a slash vs a file name;
  - config search stops at the root;
  - timeout kills the group;
  - outcome `exit code N`.
- **typecheck:**
  - `parse_problems` for tsc `(l,c)`, cargo `:l:c`, swift duplicates, warnings skipped, continuation lines;
  - check detection: package script, solution tsconfig, Cargo, go.mod, Package.swift, xcodeproj;
  - `decide_gate`: first run, "earlier remain", limit of 3 rounds.
- **settle (app-core, fake provider as in `agent_tests.rs`):**
  - a temp repo with a failing hook: the turn continues with a system message, then stops after 3 rounds;
  - a turn without edits runs nothing;
  - an aborted turn keeps its files.
- **title:** `with_project` and the budget, `clean`. **TUI:** a rename happens once, and never over an existing `custom_title`.
- **/window:**
  - the server rejects a value above the route's window;
  - the override drives the compaction budget;
  - a model switch clears it.
- **skills:**
  - a disabled skill is missing from the prompt, invocation and `skill` tool;
  - `/skills off` writes the config;
  - count and format for `/skill-usage`.
- **usagefleet:**
  - `format_group_limits`, `block_message`, `resolve_token`, stale limits;
  - the footer shows the group and hides it with the overview;
  - a submit is refused when `blocked`.
- **Every new test must fail without its change.**

## Verification
- `cargo test -p pj-ext`, `-p jcode-app-core`, `-p jcode-tui`, `-p jcode-base` and `-p jcode --lib`, compared with `/tmp/fail-base.txt`: no new failures.
- `cargo fmt --check` on the changed files.
- `cargo build --release --bin pj`. The symlink `~/.local/bin/pj` is already in place.
- **Manual checks with the release binary:**
  - `pj run` in a temp Cargo project with a deliberate type error: the agent gets "Typecheck of the projects you changed found 1 new error" and fixes it.
  - An `.agents/edit-hooks.json` with `false`: three rounds, then "stopped retrying".
  - A new session: the tab changes from `<dir> | pj` to `<dir>｜<topic> | pj`.
  - `/window 200k` changes `ctx %` in the footer.
  - `/skills off <x>`, then `/x` refuses.

## Assumptions and defaults
- One shared fix-round counter (3 per user prompt) for hooks and typecheck. pi had two separate counters.
- Esc waits for the hooks and the typecheck to finish (120 s timeout), as in pi. This is a `shortcut:` comment in the code.
- usagefleet blocks using the latest fetched state, not a fresh request per prompt: the first prompt right after a block starts can still go through. There is no claude-accounts pin logic, so the account is the collector's.
- The skill log is shared with pi (`~/.cache/pi-skill-usage.jsonl`).
- The title model is `anthropic/claude-haiku-4-5`, the same as condense. Change it in `[pj.title] model`.
- The "covered" list above rests on jcode features found in the code. If one of them turns out not to cover the pi extension, it moves to M6.
