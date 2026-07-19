//! Pure parsing/classification of the Linux kernel's `struct input_event`
//! wire format (`linux/input.h`), split from `evdev_counter.rs`'s actual
//! file reading so the parsing logic is testable without a real
//! `/dev/input/event*` device node — this environment's WSL sandbox has
//! no such device nodes at all (no kernel input devices are exposed to
//! the VM), so this split is not just good practice here, it's the only
//! way to exercise this logic in this dev environment at all (see
//! `evdev_counter.rs`'s doc comment for how OS-facing reading was
//! verified instead).
//!
//! On x86_64 Linux, `struct input_event` is exactly 24 bytes: an 8-byte
//! `tv_sec`, an 8-byte `tv_usec`, a 2-byte `type`, a 2-byte `code`, and a
//! 4-byte `value` — native byte order, no padding (already 8-byte
//! aligned). This is a kernel ABI, not expected to change.

pub const INPUT_EVENT_SIZE: usize = 24;

const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;

const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;

/// First button code (`BTN_MISC`) — codes below this in the `EV_KEY`
/// range are keyboard keys; this code and above are buttons (mouse,
/// joystick, etc.). Mirrors the kernel's own `input-event-codes.h`
/// range convention.
const BTN_MISC: u16 = 0x100;

/// `value == 1` is "key/button pressed" in the kernel's `input_event`
/// convention (0 = released, 2 = autorepeat) — counting only presses,
/// never releases or autorepeat, mirrors `windows-collector::hooks`'s
/// `WM_KEYDOWN`-only counting exactly (one count per physical
/// press, not per up/down pair, and no autorepeat inflation).
const KEY_PRESSED: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawInputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Keyboard,
    MouseMove,
    MouseClick,
}

/// Parses one 24-byte little-endian record. Returns `None` for a
/// too-short slice (the caller's read loop is responsible for only ever
/// handing this whole, aligned records — see `evdev_counter.rs`).
pub fn parse_input_event(bytes: &[u8]) -> Option<RawInputEvent> {
    if bytes.len() < INPUT_EVENT_SIZE {
        return None;
    }
    let event_type = u16::from_ne_bytes([bytes[16], bytes[17]]);
    let code = u16::from_ne_bytes([bytes[18], bytes[19]]);
    let value = i32::from_ne_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some(RawInputEvent {
        event_type,
        code,
        value,
    })
}

/// Classifies one already-parsed event, or `None` for anything this
/// collector doesn't count (`EV_SYN` frame separators, `EV_ABS`,
/// key/button releases, autorepeat, etc.) — mirrors
/// `windows-collector::hooks`'s hook procedures' own `match` exactly:
/// increment on specific, narrow conditions, ignore everything else.
pub fn classify_event(event: RawInputEvent) -> Option<EventKind> {
    match event.event_type {
        EV_KEY if event.value == KEY_PRESSED => {
            if event.code < BTN_MISC {
                Some(EventKind::Keyboard)
            } else {
                Some(EventKind::MouseClick)
            }
        }
        EV_REL if event.code == REL_X || event.code == REL_Y => Some(EventKind::MouseMove),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(event_type: u16, code: u16, value: i32) -> [u8; INPUT_EVENT_SIZE] {
        let mut buf = [0u8; INPUT_EVENT_SIZE];
        buf[16..18].copy_from_slice(&event_type.to_ne_bytes());
        buf[18..20].copy_from_slice(&code.to_ne_bytes());
        buf[20..24].copy_from_slice(&value.to_ne_bytes());
        buf
    }

    #[test]
    fn parses_a_well_formed_record() {
        let buf = encode(EV_KEY, 30, 1); // KEY_A press
        let event = parse_input_event(&buf).unwrap();
        assert_eq!(event.event_type, EV_KEY);
        assert_eq!(event.code, 30);
        assert_eq!(event.value, 1);
    }

    #[test]
    fn too_short_slice_returns_none() {
        assert!(parse_input_event(&[0u8; 10]).is_none());
    }

    #[test]
    fn key_press_below_btn_misc_is_keyboard() {
        let event = parse_input_event(&encode(EV_KEY, 30, KEY_PRESSED)).unwrap(); // KEY_A
        assert_eq!(classify_event(event), Some(EventKind::Keyboard));
    }

    #[test]
    fn key_release_is_not_counted() {
        let event = parse_input_event(&encode(EV_KEY, 30, 0)).unwrap();
        assert_eq!(classify_event(event), None);
    }

    #[test]
    fn autorepeat_is_not_counted() {
        let event = parse_input_event(&encode(EV_KEY, 30, 2)).unwrap();
        assert_eq!(classify_event(event), None);
    }

    #[test]
    fn button_press_at_or_above_btn_misc_is_mouse_click() {
        let event = parse_input_event(&encode(EV_KEY, 0x110, KEY_PRESSED)).unwrap(); // BTN_LEFT
        assert_eq!(classify_event(event), Some(EventKind::MouseClick));
    }

    #[test]
    fn rel_x_and_rel_y_are_mouse_move() {
        let x = parse_input_event(&encode(EV_REL, REL_X, 5)).unwrap();
        let y = parse_input_event(&encode(EV_REL, REL_Y, -3)).unwrap();
        assert_eq!(classify_event(x), Some(EventKind::MouseMove));
        assert_eq!(classify_event(y), Some(EventKind::MouseMove));
    }

    #[test]
    fn other_rel_axes_are_not_counted() {
        const REL_WHEEL: u16 = 0x08;
        let event = parse_input_event(&encode(EV_REL, REL_WHEEL, 1)).unwrap();
        assert_eq!(classify_event(event), None);
    }

    #[test]
    fn ev_syn_frame_separators_are_not_counted() {
        const EV_SYN: u16 = 0x00;
        let event = parse_input_event(&encode(EV_SYN, 0, 0)).unwrap();
        assert_eq!(classify_event(event), None);
    }
}
