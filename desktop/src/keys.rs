//! Translates native key presses into lazygit input. Keys that lazygit has
//! names for are sent as key labels in the format of lazygit's keybinding
//! config, so lazygit's own (and the user's custom) keybindings handle them;
//! typed characters are sent as text, which lazygit handles exactly like the
//! corresponding single-character key labels, but which also covers characters
//! that labels can't express, like those typed with Option on macOS.

use crate::protocol::Outgoing;
use gpui::Keystroke;

#[derive(Debug, PartialEq)]
pub enum KeyAction {
    Send(Outgoing),
    Paste,
    Quit,
}

/// `editing` says whether the focused lazygit view is a text field, where
/// Option-modified characters are text rather than Alt shortcuts.
pub fn key_action(stroke: &Keystroke, editing: bool) -> Option<KeyAction> {
    let m = &stroke.modifiers;
    if m.platform && !m.control && !m.alt {
        match stroke.key.as_str() {
            "v" => return Some(KeyAction::Paste),
            "q" => return Some(KeyAction::Quit),
            // Leave the other standard window shortcuts to the system.
            "w" | "h" | "m" | "`" | "tab" => return None,
            _ => {}
        }
    }
    if m.function && named_key(&stroke.key).is_none() {
        return None;
    }
    // Command is the key that terminals on macOS report as meta, which is
    // what lazygit's macOS defaults (like <meta+enter>) are written for.
    let meta = m.platform;

    if let Some(name) = named_key(&stroke.key) {
        if name == "space" && !m.control && !m.alt && !meta {
            return Some(text(" "));
        }
        let name = if name == "tab" && m.shift { "backtab" } else { name };
        let shift = m.shift && name != "backtab";
        return Some(label(m.control, m.alt, shift, meta, name));
    }

    let key = stroke.key.as_str();
    if key.chars().count() != 1 {
        return None;
    }
    let key = match key {
        "-" => "minus",
        "+" => "plus",
        key => key,
    };
    if m.control || meta {
        return Some(label(m.control, m.alt, m.shift, meta, &key.to_lowercase()));
    }
    let typed = stroke.key_char.as_deref().filter(|c| !c.is_empty());
    if m.alt && (!editing || typed.is_none()) {
        return Some(label(false, true, m.shift, false, &key.to_lowercase()));
    }
    Some(text(typed.unwrap_or(stroke.key.as_str())))
}

fn named_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "escape" => "esc",
        "enter" => "enter",
        "backspace" => "backspace",
        "delete" => "delete",
        "insert" => "insert",
        "tab" => "tab",
        "space" => "space",
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        "home" => "home",
        "end" => "end",
        "pageup" => "pgup",
        "pagedown" => "pgdown",
        "f1" => "f1",
        "f2" => "f2",
        "f3" => "f3",
        "f4" => "f4",
        "f5" => "f5",
        "f6" => "f6",
        "f7" => "f7",
        "f8" => "f8",
        "f9" => "f9",
        "f10" => "f10",
        "f11" => "f11",
        "f12" => "f12",
        _ => return None,
    })
}

fn label(control: bool, alt: bool, shift: bool, meta: bool, key: &str) -> KeyAction {
    let mut label = String::new();
    if control {
        label += "c-";
    }
    if alt {
        label += "a-";
    }
    if shift {
        label += "s-";
    }
    if meta {
        label += "m-";
    }
    label += key;
    KeyAction::Send(Outgoing::Key {
        key: format!("<{label}>"),
    })
}

fn text(text: &str) -> KeyAction {
    KeyAction::Send(Outgoing::Text { text: text.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(stroke: &str, key_char: Option<&str>, editing: bool) -> Option<KeyAction> {
        let mut stroke = Keystroke::parse(stroke).unwrap();
        stroke.key_char = key_char.map(Into::into);
        key_action(&stroke, editing)
    }

    fn key(label: &str) -> Option<KeyAction> {
        Some(KeyAction::Send(Outgoing::Key { key: label.into() }))
    }

    fn typed(text: &str) -> Option<KeyAction> {
        Some(KeyAction::Send(Outgoing::Text { text: text.into() }))
    }

    #[test]
    fn named_keys_become_labels() {
        assert_eq!(action("enter", None, false), key("<enter>"));
        assert_eq!(action("escape", None, false), key("<esc>"));
        assert_eq!(action("shift-tab", None, false), key("<backtab>"));
        assert_eq!(action("pagedown", None, false), key("<pgdown>"));
        assert_eq!(action("shift-down", None, false), key("<s-down>"));
        assert_eq!(action("alt-backspace", None, true), key("<a-backspace>"));
        assert_eq!(action("ctrl-enter", None, false), key("<c-enter>"));
    }

    #[test]
    fn characters_become_text() {
        assert_eq!(action("a", Some("a"), false), typed("a"));
        assert_eq!(action("shift-a", Some("A"), false), typed("A"));
        assert_eq!(action("shift-/", Some("?"), false), typed("?"));
        assert_eq!(action("space", Some(" "), true), typed(" "));
        assert_eq!(action("alt-a", Some("å"), true), typed("å"));
    }

    #[test]
    fn modified_characters_become_labels() {
        assert_eq!(action("ctrl-r", None, false), key("<c-r>"));
        assert_eq!(action("ctrl-shift-a", None, false), key("<c-s-a>"));
        assert_eq!(action("alt-a", Some("å"), false), key("<a-a>"));
        assert_eq!(action("ctrl--", None, false), key("<c-minus>"));
    }

    #[test]
    fn platform_shortcuts_stay_native() {
        assert_eq!(action("cmd-v", None, false), Some(KeyAction::Paste));
        assert_eq!(action("cmd-q", None, false), Some(KeyAction::Quit));
        assert_eq!(action("cmd-w", None, false), None);
    }

    #[test]
    fn command_is_meta() {
        assert_eq!(action("cmd-enter", None, true), key("<m-enter>"));
        assert_eq!(action("cmd-k", None, false), key("<m-k>"));
    }
}
