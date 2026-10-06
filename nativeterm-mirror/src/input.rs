//! A device's input, applied to a pane as the desktop's own keyboard
//! would: text as typed (an input method's result), a paste through the
//! pane's paste (bracketed when the program asked for it), a key by name
//! encoded by WezTerm for the pane's modes (application cursor keys, the
//! kitty keyboard protocol, ...). (NativeTerm keeps the log of it.)

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
