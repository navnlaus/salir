//! Keys as CEF wants them: Windows' virtual key codes (what Chromium's
//! KeyboardCode is), whatever the system, and the system's own code beside it.
//! (Text typed goes as text: Page::text.)

use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;

/// The key's code as the layout has it (`logical`: a French A is A wherever it is),
/// else by where it is on the keyboard.
pub fn windows_key_code(logical: &Key, physical: PhysicalKey) -> i32 {
    let by_layout = match logical {
        Key::Character(s) => match s.chars().next().map(|c| c.to_ascii_uppercase()) {
            Some(c @ ('A'..='Z' | '0'..='9')) => c as i32,
            Some(' ') => 0x20,
            Some('-' | '_') => 0xbd,
            Some('=' | '+') => 0xbb,
            Some(',' | '<') => 0xbc,
            Some('.' | '>') => 0xbe,
            Some('/' | '?') => 0xbf,
            Some(';' | ':') => 0xba,
            Some('\'' | '"') => 0xde,
            Some('[' | '{') => 0xdb,
            Some(']' | '}') => 0xdd,
            Some('\\' | '|') => 0xdc,
            Some('`' | '~') => 0xc0,
            _ => 0,
        },
        Key::Named(named) => match named {
            NamedKey::Enter => 0x0d,
            NamedKey::Backspace => 0x08,
            NamedKey::Tab => 0x09,
            NamedKey::Escape => 0x1b,
            NamedKey::Space => 0x20,
            NamedKey::ArrowLeft => 0x25,
            NamedKey::ArrowUp => 0x26,
            NamedKey::ArrowRight => 0x27,
            NamedKey::ArrowDown => 0x28,
            NamedKey::Home => 0x24,
            NamedKey::End => 0x23,
            NamedKey::PageUp => 0x21,
            NamedKey::PageDown => 0x22,
            NamedKey::Delete => 0x2e,
            NamedKey::Insert => 0x2d,
            NamedKey::Shift => 0x10,
            NamedKey::Control => 0x11,
            NamedKey::Alt => 0x12,
            _ => 0,
        },
        _ => 0,
    };
    if by_layout != 0 { by_layout } else { physical_key_code(physical) }
}

fn physical_key_code(key: PhysicalKey) -> i32 {
    let PhysicalKey::Code(code) = key else { return 0 };
    use KeyCode::*;
    match code {
        KeyA => 0x41, KeyB => 0x42, KeyC => 0x43, KeyD => 0x44, KeyE => 0x45, KeyF => 0x46, KeyG => 0x47,
        KeyH => 0x48, KeyI => 0x49, KeyJ => 0x4a, KeyK => 0x4b, KeyL => 0x4c, KeyM => 0x4d, KeyN => 0x4e,
        KeyO => 0x4f, KeyP => 0x50, KeyQ => 0x51, KeyR => 0x52, KeyS => 0x53, KeyT => 0x54, KeyU => 0x55,
        KeyV => 0x56, KeyW => 0x57, KeyX => 0x58, KeyY => 0x59, KeyZ => 0x5a,
        Digit0 => 0x30, Digit1 => 0x31, Digit2 => 0x32, Digit3 => 0x33, Digit4 => 0x34,
        Digit5 => 0x35, Digit6 => 0x36, Digit7 => 0x37, Digit8 => 0x38, Digit9 => 0x39,
        Backspace => 0x08, Tab => 0x09, Enter | NumpadEnter => 0x0d, ShiftLeft | ShiftRight => 0x10,
        ControlLeft | ControlRight => 0x11, AltLeft | AltRight => 0x12, Escape => 0x1b, Space => 0x20,
        PageUp => 0x21, PageDown => 0x22, End => 0x23, Home => 0x24,
        ArrowLeft => 0x25, ArrowUp => 0x26, ArrowRight => 0x27, ArrowDown => 0x28,
        Insert => 0x2d, Delete => 0x2e, Semicolon => 0xba, Equal => 0xbb, Comma => 0xbc,
        Minus => 0xbd, Period => 0xbe, Slash => 0xbf, Backquote => 0xc0, BracketLeft => 0xdb,
        Backslash => 0xdc, BracketRight => 0xdd, Quote => 0xde, NumpadAdd => 0x6b, NumpadSubtract => 0x6d,
        _ => 0,
    }
}

/// The system's code for the key: on Linux X11's keycode (evdev's + 8), which Chromium reads the key's place from.
pub fn native_key_code(key: PhysicalKey) -> i32 {
    match key.to_scancode() {
        #[cfg(target_os = "linux")]
        Some(scancode) => scancode as i32 + 8,
        #[cfg(not(target_os = "linux"))]
        Some(scancode) => scancode as i32,
        None => 0,
    }
}
