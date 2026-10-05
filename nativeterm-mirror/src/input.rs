//! A device's input, applied to a pane as the desktop's own keyboard
//! would: text as typed (an input method's result), a paste through the
//! pane's paste (bracketed when the program asked for it), a key by name
//! encoded by WezTerm for the pane's modes (application cursor keys, the
//! kitty keyboard protocol, ...). Recorded in `input.log` beside
//! `state.json`: which device, which pane, what kind, the key's name;
//! never the text (a password typed into a terminal is not echoed, and
//! the log must not keep it either).

use std::io::Write;
use std::path::Path;

use mux::pane::Pane;
use native_term_remote::keys;
use native_term_remote::proto::input::Kind;
use native_term_remote::proto::input_result::Reason;
use native_term_remote::proto::mouse::{Button, Kind as MouseKind};
use wezterm_term::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// WezTerm's key for a protocol name; `None` for one it does not know.
pub fn key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "Backspace" => KeyCode::Backspace,
        "Escape" => KeyCode::Escape,
        "Delete" => KeyCode::Delete,
        "Insert" => KeyCode::Insert,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "ArrowUp" => KeyCode::UpArrow,
        "ArrowDown" => KeyCode::DownArrow,
        "ArrowLeft" => KeyCode::LeftArrow,
        "ArrowRight" => KeyCode::RightArrow,
        _ => {
            if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
                if (1..=12).contains(&n) {
                    return Some(KeyCode::Function(n));
                }
            }
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return None,
            }
        }
    })
}

pub fn modifiers(mods: u32) -> KeyModifiers {
    let mut m = KeyModifiers::NONE;
    for (bit, modifier) in [
        (keys::SHIFT, KeyModifiers::SHIFT),
        (keys::ALT, KeyModifiers::ALT),
        (keys::CTRL, KeyModifiers::CTRL),
        (keys::SUPER, KeyModifiers::SUPER),
    ] {
        if mods & bit != 0 {
            m |= modifier;
        }
    }
    m
}

/// Apply `kind` to `pane`.
pub fn apply(pane: &dyn Pane, kind: &Kind) -> Reason {
    let done = match kind {
        Kind::Text(text) => {
            let mut writer = pane.writer();
            writer
                .write_all(text.as_bytes())
                .and_then(|()| writer.flush())
                .map_err(anyhow::Error::from)
        }
        Kind::Paste(text) => pane.send_paste(text),
        Kind::Key(key) => match key_code(&key.name) {
            Some(code) => pane.key_down(code, modifiers(key.mods)),
            None => return Reason::UnknownKey,
        },
        // only for a program that asked for it: else the device scrolls
        // and selects on its own
        Kind::Mouse(mouse) if pane.is_mouse_grabbed() => pane.mouse_event(MouseEvent {
            kind: match mouse.kind() {
                MouseKind::Press => MouseEventKind::Press,
                MouseKind::Release => MouseEventKind::Release,
                MouseKind::Move => MouseEventKind::Move,
            },
            x: mouse.col as usize,
            y: mouse.row as i64,
            x_pixel_offset: 0,
            y_pixel_offset: 0,
            button: match mouse.button() {
                Button::None => MouseButton::None,
                Button::Left => MouseButton::Left,
                Button::Middle => MouseButton::Middle,
                Button::Right => MouseButton::Right,
                Button::WheelUp => MouseButton::WheelUp(1),
                Button::WheelDown => MouseButton::WheelDown(1),
            },
            modifiers: modifiers(mouse.mods),
        }),
        Kind::Mouse(_) => Ok(()),
    };
    if let Err(e) = done {
        log::debug!("nativeterm remote control: input: {e:#}");
    }
    Reason::Applied
}

/// What the log says of an input: its kind and size, a key's name.
pub fn describe(kind: &Kind) -> String {
    match kind {
        Kind::Text(text) => format!("text, {} characters", text.chars().count()),
        Kind::Paste(text) => format!("paste, {} characters", text.chars().count()),
        Kind::Mouse(m) => format!(
            "mouse {:?} {:?} at {},{}",
            m.kind(),
            m.button(),
            m.row,
            m.col
        ),
        Kind::Key(key) => {
            let mut name = String::new();
            for (bit, label) in [
                (keys::CTRL, "Ctrl+"),
                (keys::ALT, "Alt+"),
                (keys::SHIFT, "Shift+"),
                (keys::SUPER, "Super+"),
            ] {
                if key.mods & bit != 0 {
                    name.push_str(label);
                }
            }
            name.push_str(&key.name);
            format!("key {name}")
        }
    }
}

/// One line in `input.log`.
pub fn record(dir: &Path, device: &str, pane: &str, kind: &Kind, reason: Reason) {
    let when = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let outcome = match reason {
        Reason::Applied => "",
        Reason::ReadOnly => " (refused: read only)",
        Reason::NoSession => " (refused: no such session)",
        Reason::UnknownKey => " (refused: unknown key)",
    };
    let line = format!("{when} {device} {pane} {}{outcome}\n", describe(kind));
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("input.log"))
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        log::debug!("nativeterm remote control: input.log: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_term_remote::proto::Key;

    #[test]
    fn every_protocol_key_has_a_code() {
        for name in keys::NAMED {
            assert!(key_code(name).is_some(), "{name}");
        }
        assert_eq!(key_code("F12"), Some(KeyCode::Function(12)));
        assert_eq!(key_code("中"), Some(KeyCode::Char('中')));
        assert_eq!(key_code("F13"), None);
        assert_eq!(key_code("Ctrl"), None);
        assert_eq!(
            modifiers(keys::CTRL | keys::SHIFT),
            KeyModifiers::CTRL | KeyModifiers::SHIFT
        );
    }

    #[test]
    fn the_log_never_has_the_text() {
        let typed = describe(&Kind::Text("hunter2".into()));
        assert_eq!(typed, "text, 7 characters");
        let key = Key {
            name: "c".into(),
            mods: keys::CTRL,
        };
        assert_eq!(describe(&Kind::Key(key)), "key Ctrl+c");
    }
}
