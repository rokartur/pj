//! pi's look: the fx theme (`~/.pi/agent/themes/fx.json`) as `[display.colors]`
//! defaults, so every jcode color role renders in fx's grays.

/// fx role colors for a dark terminal. A role set in `[display.colors]` wins.
pub const FX_COLORS: [(&str, &str); 22] = [
    ("user", "#eeeeee"),
    ("ai", "#d0d0d0"),
    ("tool", "#ffffff"),
    ("file_link", "#d0d0d0"),
    ("dim", "#858585"),
    ("accent", "#eeeeee"),
    ("system", "#9e9e9e"),
    ("queued", "#d7af5f"),
    ("asap", "#5fd7ff"),
    ("pending", "#58a6ff"),
    ("user_text", "#eeeeee"),
    ("user_bg", "#262626"),
    ("ai_text", "#d0d0d0"),
    ("header_icon", "#eeeeee"),
    ("header_name", "#d0d0d0"),
    ("header_session", "#ffffff"),
    ("success", "#7ee787"),
    ("warning", "#d7af5f"),
    ("error", "#ff7b72"),
    ("info", "#9e9e9e"),
    ("border", "#585858"),
    ("selection_bg", "#262626"),
];
