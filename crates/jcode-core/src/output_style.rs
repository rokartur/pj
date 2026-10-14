use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use unicode_properties::emoji::{
    EmojiStatus, UnicodeEmoji, is_emoji_presentation_selector, is_regional_indicator,
    is_text_presentation_selector, is_zwj,
};
use unicode_segmentation::UnicodeSegmentation;

static EMOJI_ENABLED: AtomicBool = AtomicBool::new(true);

#[macro_export]
macro_rules! terminal_print {
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::print!("{}", $crate::output_style::terminal_text(&message));
    }};
}

#[macro_export]
macro_rules! terminal_println {
    () => { ::std::println!() };
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::println!("{}", $crate::output_style::terminal_text(&message));
    }};
}

#[macro_export]
macro_rules! terminal_eprint {
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::eprint!("{}", $crate::output_style::terminal_text(&message));
    }};
}

#[macro_export]
macro_rules! terminal_eprintln {
    () => { ::std::eprintln!() };
    ($($arg:tt)*) => {{
        let message = ::std::format!($($arg)*);
        ::std::eprintln!("{}", $crate::output_style::terminal_text(&message));
    }};
}

/// Set whether terminal-facing output may contain emoji.
pub fn set_emoji_enabled(enabled: bool) {
    EMOJI_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Return whether terminal-facing output may contain emoji.
pub fn emoji_enabled() -> bool {
    EMOJI_ENABLED.load(Ordering::Relaxed)
}

/// Adapt terminal-facing text to the configured emoji preference.
pub fn terminal_text(text: &str) -> Cow<'_, str> {
    terminal_text_with_emoji(text, emoji_enabled())
}

/// Adapt terminal-facing text using an explicit emoji preference.
pub fn terminal_text_with_emoji(text: &str, enabled: bool) -> Cow<'_, str> {
    if enabled || text.is_ascii() || !contains_emoji(text) {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(replace_emoji_with_glyphs(text))
    }
}

/// Replace emoji grapheme clusters with Nerd Font (Material Design) glyphs.
pub fn replace_emoji_with_glyphs(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for grapheme in text.graphemes(true) {
        if grapheme_is_emoji(grapheme) {
            output.push(nerd_font_glyph(grapheme));
        } else {
            output.push_str(grapheme);
        }
    }
    output
}

fn contains_emoji(text: &str) -> bool {
    text.graphemes(true).any(grapheme_is_emoji)
}

/// Code points come from nerd-fonts glyphnames.json (`md-*` names).
fn nerd_font_glyph(grapheme: &str) -> char {
    let Some(ch) = grapheme.chars().next() else {
        return '\u{F09DF}';
    };
    match ch {
        '✓' | '✔' | '✅' => '\u{F05E0}',               // md-check_circle
        '✕' | '✗' | '❌' | '❎' => '\u{F0159}',        // md-close_circle
        '⚠' | '🚨' => '\u{F0026}',                     // md-alert
        '➡' | '👉' | '➜' | '➤' => '\u{F0054}',         // md-arrow_right
        '⬅' | '👈' => '\u{F004D}',                     // md-arrow_left
        '🐝' => '\u{F0FA1}',                           // md-bee
        '🧠' => '\u{F09D1}',                           // md-brain
        '🔍' | '🔎' => '\u{F0349}',                    // md-magnify
        '⚡' => '\u{F140B}',                           // md-lightning_bolt
        '★' | '⭐' | '✦' | '✨' | '💫' => '\u{F04CE}', // md-star
        '🛑' => '\u{F03C3}',                           // md-octagon
        '📌' | '📍' => '\u{F0403}',                    // md-pin
        '📦' => '\u{F03D6}',                           // md-package_variant
        '🙂' | '😀' => '\u{F0C71}',                    // md-emoticon_happy
        '🚀' => '\u{F14DE}',                           // md-rocket_launch
        '🦀' => '\u{F1617}',                           // md-language_rust
        '🔥' => '\u{F0238}',                           // md-fire
        '🦊' => '\u{F0239}',                           // md-firefox
        '⚙' | '⛭' => '\u{F0493}',                      // md-cog
        '📋' => '\u{F014D}',                           // md-clipboard_text
        '🔄' => '\u{F04E6}',                           // md-sync
        '♻' => '\u{F044C}',                            // md-recycle
        '🐙' => '\u{F02A4}',                           // md-github
        '💥' => '\u{F0691}',                           // md-bomb
        '💬' => '\u{F0361}',                           // md-message
        '💭' => '\u{F07F6}',                           // md-thought_bubble
        '🌿' | '🌱' | '🍀' => '\u{F0E66}',             // md-sprout
        '🧵' => '\u{F0391}',                           // md-needle
        '✂' => '\u{F0190}',                            // md-content_cut
        '🌙' => '\u{F0594}',                           // md-weather_night
        '💡' => '\u{F0335}',                           // md-lightbulb
        '🤖' => '\u{F06A9}',                           // md-robot
        '🧊' => '\u{F0717}',                           // md-snowflake
        '♥' | '🤍' => '\u{F02D1}',                     // md-heart
        '🎯' => '\u{F04FE}',                           // md-target
        '✎' | '✍' | '📝' => '\u{F03EB}',               // md-pencil
        '🎙' | '🎤' => '\u{F036C}',                     // md-microphone
        '🎬' => '\u{F0FCE}',                           // md-movie_open
        '📁' => '\u{F024B}',                           // md-folder
        '🗂' => '\u{F12E3}',                            // md-folder_table
        '🔮' => '\u{F0B2F}',                           // md-crystal_ball
        '🛠' | '🔧' | '🔨' => '\u{F1064}',              // md-tools
        '🧰' => '\u{F09AC}',                           // md-toolbox
        '☁' => '\u{F015F}',                            // md-cloud
        '🌫' => '\u{F0591}',                            // md-weather_fog
        '🌲' | '🌳' => '\u{F0531}',                    // md-tree
        '🌴' => '\u{F1055}',                           // md-palm_tree
        '🌵' => '\u{F0DB5}',                           // md-cactus
        '🌷' | '🌸' | '🌹' | '🌺' | '🌻' | '🌼' => '\u{F09F1}', // md-flower_tulip
        '🦉' => '\u{F03D2}',                           // md-owl
        '👤' => '\u{F0004}',                           // md-account
        '💧' => '\u{F058C}',                           // md-water
        '📊' | '📈' => '\u{F0128}',                    // md-chart_bar
        '📡' => '\u{F1119}',                           // md-antenna
        '📣' => '\u{F00E6}',                           // md-bullhorn
        '🖼' | '📷' => '\u{F0100}',                     // md-camera
        '⚑' => '\u{F023B}',                            // md-flag
        '⚓' => '\u{F0031}',                           // md-anchor
        '✉' => '\u{F01EE}',                            // md-email
        '🌍' => '\u{F01E7}',                           // md-earth
        '🌐' => '\u{F059F}',                           // md-web
        '🎉' => '\u{F1056}',                           // md-party_popper
        '💰' => '\u{F0114}',                           // md-cash
        '💾' => '\u{F0193}',                           // md-content_save
        '📅' => '\u{F00ED}',                           // md-calendar
        '🔗' => '\u{F0337}',                           // md-link
        '🔬' => '\u{F0654}',                           // md-microscope
        '🔭' => '\u{F0B4E}',                           // md-telescope
        '🕒' => '\u{F0150}',                           // md-clock_outline
        '🗑' => '\u{F01B4}',                            // md-delete
        '🏷' => '\u{F04F9}',                            // md-tag
        '🧩' => '\u{F0431}',                           // md-puzzle
        '🧪' => '\u{F0668}',                           // md-test_tube
        '🧭' => '\u{F018B}',                           // md-compass
        '🏠' => '\u{F02DC}',                           // md-home
        '🏰' | '🏯' => '\u{F011A}',                    // md-castle
        '🏭' => '\u{F020F}',                           // md-factory
        '⛺' | '🏕' => '\u{F0508}',                     // md-tent
        '⛪' => '\u{F0144}',                           // md-church
        '⛲' => '\u{F096B}',                           // md-fountain
        '🌉' => '\u{F0618}',                           // md-bridge
        '🌊' => '\u{F0F2E}',                           // md-wave
        '🍄' => '\u{F07DF}',                           // md-mushroom
        '🛶' | '🚣' => '\u{F0608}',                    // md-rowing
        '🚉' => '\u{F052C}',                           // md-train
        '🚠' => '\u{F0686}',                           // md-gondola
        '🗻' => '\u{F02F5}',                           // md-image_filter_hdr
        '👋' => '\u{F1821}',                           // md-hand_wave
        '👨' | '👦' => '\u{F064D}',                    // md-human_male
        '👩' | '👧' => '\u{F0649}',                    // md-human_female
        '🙋' => '\u{F064C}',                           // md-human_handsup
        '🏄' => '\u{F1746}',                           // md-surfing
        '🐈' | '🐱' => '\u{F011B}',                    // md-cat
        '🐕' | '🐶' | '🐩' | '🦮' => '\u{F0A43}',      // md-dog
        '🐟' | '🐠' | '🐡' => '\u{F023A}',             // md-fish
        '🐧' => '\u{F0EC0}',                           // md-penguin
        '🐇' | '🐰' => '\u{F0907}',                    // md-rabbit
        '🐍' => '\u{F150E}',                           // md-snake
        '🐢' => '\u{F0CD7}',                           // md-turtle
        '🦆' => '\u{F01E5}',                           // md-duck
        '🐘' | '🦣' => '\u{F07C6}',                    // md-elephant
        '🐖' | '🐷' | '🐽' | '🐗' => '\u{F0401}',      // md-pig
        '🐄' | '🐮' | '🐂' | '🐃' | '🦬' => '\u{F019A}', // md-cow
        '🐎' | '🐴' | '🦄' => '\u{F15BF}',             // md-horse
        '🐦' | '🐤' | '🐥' | '🐣' | '🦅' | '🦜' | '🦚' | '🦢' | '🦩' | '🦤' | '🐓' | '🐔' =>
        {
            '\u{F15C6}' // md-bird
        }
        '🦃' => '\u{F171B}',                                    // md-turkey
        '🦂' | '🦗' | '🦟' | '🐜' | '🐛' | '🐞' => '\u{F00E4}', // md-bug
        '🦋' => '\u{F1589}',                                    // md-butterfly
        '🐬' => '\u{F18B4}',                                    // md-dolphin
        '🦈' => '\u{F18BA}',                                    // md-shark
        '🦘' => '\u{F1558}',                                    // md-kangaroo
        '🐀' | '🐁' | '🐭' | '🐹' | '🦫' | '🦔' => '\u{F1327}', // md-rodent
        '🐌' | '🐚' => '\u{F1677}',                             // md-snail
        '🦇' => '\u{F0B5F}',                                    // md-bat
        '🐑' | '🐏' | '🐐' | '🦙' => '\u{F0CC6}',               // md-sheep
        '🐼' => '\u{F03DA}',                                    // md-panda
        '🐨' => '\u{F173F}',                                    // md-koala
        '🐻' => '\u{F18FB}',                                    // md-teddy_bear
        '🦴' => '\u{F00B9}',                                    // md-bone
        '🦷' => '\u{F08C3}',                                    // md-tooth
        '🦠' => '\u{F0ED5}',                                    // md-bacteria
        '🐾' => '\u{F03E9}',                                    // md-paw
        ch if ('\u{1F400}'..='\u{1F43F}').contains(&ch)
            || ('\u{1F980}'..='\u{1F9AF}').contains(&ch) =>
        {
            '\u{F03E9}' // md-paw for the remaining animals
        }
        _ => '\u{F09DF}', // md-circle_small
    }
}

fn grapheme_is_emoji(grapheme: &str) -> bool {
    let has_text_selector = grapheme.chars().any(is_text_presentation_selector);
    let has_emoji_selector = grapheme.chars().any(is_emoji_presentation_selector);
    if has_text_selector && !has_emoji_selector {
        return false;
    }

    let has_emoji_char = grapheme.chars().any(UnicodeEmoji::is_emoji_char);
    let regional_indicators = grapheme
        .chars()
        .filter(|ch| is_regional_indicator(*ch))
        .count();
    has_emoji_selector
        || grapheme.contains('\u{20E3}')
        || regional_indicators >= 2
        || (has_emoji_char && grapheme.chars().any(is_zwj))
        || grapheme.chars().any(|ch| {
            matches!(
                ch.emoji_status(),
                EmojiStatus::EmojiPresentation
                    | EmojiStatus::EmojiPresentationAndModifierBase
                    | EmojiStatus::EmojiPresentationAndEmojiComponent
                    | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_clusters_become_nerd_font_glyphs() {
        assert_eq!(
            replace_emoji_with_glyphs(
                "🐝 ready ✅ warning ⚠️ failed ❌ family 👨‍👩‍👧‍👦 tone 👋🏽 flag 🇺🇸 key 1️⃣"
            ),
            "\u{F0FA1} ready \u{F05E0} warning \u{F0026} failed \u{F0159} family \u{F064D} tone \u{F1821} flag \u{F09DF} key \u{F09DF}"
        );
    }

    #[test]
    fn non_emoji_unicode_is_preserved() {
        assert_eq!(
            replace_emoji_with_glyphs("box ─│ arrows →←↔ CJK 中文 math α © ® ✓ ✗ ⚠"),
            "box ─│ arrows →←↔ CJK 中文 math α © ® ✓ ✗ ⚠"
        );
        assert_eq!(replace_emoji_with_glyphs("text heart ♥︎"), "text heart ♥︎");
    }
}
