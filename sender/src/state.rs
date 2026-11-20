#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! What the sender sends, and how Linux's input events (evdev) become it.
//! Plain code (no devices): it builds and is tested on any system.
//!
//! `Desk` and `Pad` are Theater Puppet's (src/tracking/desk.rs, gamepad.rs):
//! the same fields, the same JSON. The keys are Windows' virtual-key codes,
//! as Puppet's desk reads them; Linux's key codes are turned into them here.

use serde::{Deserialize, Serialize};

/// The keys and mouse now (Puppet's tracking::desk::Desk)
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Desk {
    /// Virtual-key codes down (a bit each, 0–255)
    pub keys: [u64; 4],
    /// Key presses so far
    pub presses: u32,
    /// The mouse moved from the middle of its pad: (right, away), metres
    pub mouse: [f32; 2],
    /// 1 left, 2 right, 4 middle
    pub buttons: u8,
    pub mouse_idle: f32,
    pub keys_idle: f32,
}

impl Desk {
    #[cfg(test)]
    pub fn down(&self, vk: u8) -> bool {
        self.keys[(vk >> 6) as usize] & (1 << (vk & 63)) != 0
    }

    fn press(&mut self, vk: u8) {
        self.keys[(vk >> 6) as usize] |= 1 << (vk & 63);
    }
}

/// A controller now (Puppet's tracking::gamepad::Pad)
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    /// x right, y up, -1–1
    pub left_stick: [f32; 2],
    pub right_stick: [f32; 2],
}

/// Puppet's buttons (gamepad.rs `button`)
pub mod button {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LB: u32 = 1 << 4;
    pub const RB: u32 = 1 << 5;
    pub const BACK: u32 = 1 << 6;
    pub const START: u32 = 1 << 7;
    pub const LS: u32 = 1 << 8;
    pub const RS: u32 = 1 << 9;
    pub const UP: u32 = 1 << 10;
    pub const DOWN: u32 = 1 << 11;
    pub const LEFT: u32 = 1 << 12;
    pub const RIGHT: u32 = 1 << 13;
    pub const GUIDE: u32 = 1 << 14;
}

/// What the sender sends (Puppet's tracking::network::Message)
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Message {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hello: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desk: Option<Desk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pad: Option<Pad>,
}

// Linux's event types and codes (linux/input-event-codes.h)
pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_SOUTH: u16 = 0x130;
pub const KEY_A: u16 = 30;
pub const KEY_SPACE: u16 = 57;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_GAS: u16 = 0x09;
pub const ABS_BRAKE: u16 = 0x0a;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// A Linux key code as Windows' virtual-key code (the keys a desk has; None: not one of them)
pub fn vk_of(code: u16) -> Option<u8> {
    Some(match code {
        1 => 0x1B,                                   // Esc
        2..=10 => 0x31 + (code - 2) as u8,           // 1–9
        11 => 0x30,                                  // 0
        12 => 0xBD,                                  // -
        13 => 0xBB,                                  // =
        14 => 0x08,                                  // Backspace
        15 => 0x09,                                  // Tab
        16 => b'Q', 17 => b'W', 18 => b'E', 19 => b'R', 20 => b'T', 21 => b'Y', 22 => b'U', 23 => b'I', 24 => b'O', 25 => b'P',
        26 => 0xDB,                                  // [
        27 => 0xDD,                                  // ]
        28 => 0x0D,                                  // Enter
        29 => 0xA2,                                  // Left Ctrl
        30 => b'A', 31 => b'S', 32 => b'D', 33 => b'F', 34 => b'G', 35 => b'H', 36 => b'J', 37 => b'K', 38 => b'L',
        39 => 0xBA,                                  // ;
        40 => 0xDE,                                  // '
        41 => 0xC0,                                  // `
        42 => 0xA0,                                  // Left Shift
        43 => 0xDC,                                  // backslash
        44 => b'Z', 45 => b'X', 46 => b'C', 47 => b'V', 48 => b'B', 49 => b'N', 50 => b'M',
        51 => 0xBC,                                  // ,
        52 => 0xBE,                                  // .
        53 => 0xBF,                                  // /
        54 => 0xA1,                                  // Right Shift
        55 => 0x6A,                                  // keypad *
        56 => 0xA4,                                  // Left Alt
        57 => 0x20,                                  // Space
        58 => 0x14,                                  // Caps Lock
        59..=68 => 0x70 + (code - 59) as u8,         // F1–F10
        69 => 0x90,                                  // Num Lock
        70 => 0x91,                                  // Scroll Lock
        71 => 0x67, 72 => 0x68, 73 => 0x69, 74 => 0x6D, // keypad 7 8 9 -
        75 => 0x64, 76 => 0x65, 77 => 0x66, 78 => 0x6B, // keypad 4 5 6 +
        79 => 0x61, 80 => 0x62, 81 => 0x63, 82 => 0x60, 83 => 0x6E, // keypad 1 2 3 0 .
        87 => 0x7A, 88 => 0x7B,                      // F11 F12
        96 => 0x0D,                                  // keypad Enter
        97 => 0xA3,                                  // Right Ctrl
        98 => 0x6F,                                  // keypad /
        100 => 0xA5,                                 // Right Alt
        102 => 0x24, 103 => 0x26, 104 => 0x21,       // Home Up PageUp
        105 => 0x25, 106 => 0x27,                    // Left Right
        107 => 0x23, 108 => 0x28, 109 => 0x22,       // End Down PageDown
        110 => 0x2D, 111 => 0x2E,                    // Insert Delete
        119 => 0x13,                                 // Pause
        125 => 0x5B, 126 => 0x5C,                    // Left, Right Super (Windows keys)
        127 => 0x5D,                                 // Menu
        _ => return None,
    })
}

/// The modifier a left or right one also is (Windows has both: Shift and Left Shift…)
fn generic(vk: u8) -> Option<u8> {
    match vk {
        0xA0 | 0xA1 => Some(0x10),
        0xA2 | 0xA3 => Some(0x11),
        0xA4 | 0xA5 => Some(0x12),
        _ => None,
    }
}

/// How far a mouse count moves the avatar's mouse (an 800 DPI mouse's), how
/// far it goes from the middle, how fast it settles back: Puppet's desk.rs
const METRES_PER_COUNT: f32 = 0.0254 / 800.0;
const REACH: [f32; 2] = [0.07, 0.05];
const SETTLE: f32 = 0.6;

/// The keyboards and mice (all of them as one)
#[derive(Clone, Debug)]
pub struct KeyboardMouse {
    /// Linux key codes down (0–767)
    down: [u64; 12],
    presses: u32,
    moved: [i32; 2],
    buttons: u8,
    mouse: [f32; 2],
    mouse_idle: f32,
    keys_idle: f32,
}

impl Default for KeyboardMouse {
    fn default() -> Self {
        KeyboardMouse { down: [0; 12], presses: 0, moved: [0, 0], buttons: 0, mouse: [0.0; 2], mouse_idle: 10.0, keys_idle: 10.0 }
    }
}

impl KeyboardMouse {
    /// One event from a keyboard or a mouse
    pub fn event(&mut self, kind: u16, code: u16, value: i32) {
        match (kind, code) {
            (EV_REL, REL_X) => {
                self.moved[0] += value;
                self.mouse_idle = 0.0;
            }
            (EV_REL, REL_Y) => {
                self.moved[1] += value;
                self.mouse_idle = 0.0;
            }
            (EV_KEY, BTN_LEFT | BTN_RIGHT | BTN_MIDDLE) => {
                let bit = match code {
                    BTN_LEFT => 1,
                    BTN_RIGHT => 2,
                    _ => 4,
                };
                if value != 0 { self.buttons |= bit } else { self.buttons &= !bit }
                self.mouse_idle = 0.0;
            }
            (EV_KEY, c) if (c as usize) < 768 => {
                let (word, bit) = ((c >> 6) as usize, 1u64 << (c & 63));
                match value {
                    // Down (a repeat, 2, changes nothing)
                    1 => {
                        if self.down[word] & bit == 0 {
                            self.presses = self.presses.wrapping_add(1);
                        }
                        self.down[word] |= bit;
                        self.keys_idle = 0.0;
                    }
                    0 => self.down[word] &= !bit,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// A device went (unplugged): what it held down is let go
    pub fn release_all(&mut self) {
        self.down = [0; 12];
        self.buttons = 0;
    }

    /// `dt` seconds on: the desk now
    pub fn tick(&mut self, dt: f32) -> Desk {
        let k = (dt / SETTLE).min(1.0);
        let moved = std::mem::take(&mut self.moved);
        let x = self.mouse[0] * (1.0 - k) + moved[0] as f32 * METRES_PER_COUNT;
        let y = self.mouse[1] * (1.0 - k) - moved[1] as f32 * METRES_PER_COUNT;
        self.mouse = [x.clamp(-REACH[0], REACH[0]), y.clamp(-REACH[1], REACH[1])];
        self.mouse_idle += dt;
        self.keys_idle += dt;
        let mut desk = Desk { presses: self.presses, mouse: self.mouse, buttons: self.buttons, mouse_idle: self.mouse_idle, keys_idle: self.keys_idle, ..Default::default() };
        for word in 0..12 {
            let mut bits = self.down[word];
            while bits != 0 {
                let code = (word * 64) as u16 + bits.trailing_zeros() as u16;
                bits &= bits - 1;
                if let Some(vk) = vk_of(code) {
                    desk.press(vk);
                    if let Some(g) = generic(vk) {
                        desk.press(g);
                    }
                }
            }
        }
        // The mouse's buttons as Windows has them (1 left, 2 right, 4 middle)
        for (bit, vk) in [(1, 0x01), (2, 0x02), (4, 0x04)] {
            if self.buttons & bit != 0 {
                desk.press(vk);
            }
        }
        desk
    }
}

/// A controller (one: the first found)
#[derive(Clone, Debug, Default)]
pub struct Gamepad {
    buttons: u32,
    /// Each axis: value, min, max (from the device's own ranges)
    axes: [(i32, i32, i32); 0x20],
    /// The right stick is RX/RY (most drivers), else Z/RZ (some older ones)
    right_on_rx: bool,
    /// Triggers as buttons (digital ones), when they have no axes
    trigger_buttons: [bool; 2],
}

impl Gamepad {
    /// A controller with these axes' ranges ((code, min, max))
    pub fn new(ranges: &[(u16, i32, i32)]) -> Gamepad {
        let mut g = Gamepad::default();
        for &(code, min, max) in ranges {
            if (code as usize) < g.axes.len() {
                g.axes[code as usize] = ((min + max) / 2, min, max);
            }
        }
        // Triggers rest at their minimum
        for code in [ABS_GAS, ABS_BRAKE] {
            let a = &mut g.axes[code as usize];
            a.0 = a.1;
        }
        g.right_on_rx = ranges.iter().any(|r| r.0 == ABS_RX);
        if g.right_on_rx {
            for code in [ABS_Z, ABS_RZ] {
                let a = &mut g.axes[code as usize];
                a.0 = a.1;
            }
        }
        g
    }

    pub fn event(&mut self, kind: u16, code: u16, value: i32) {
        match kind {
            EV_ABS if (code as usize) < self.axes.len() => self.axes[code as usize].0 = value,
            EV_KEY => {
                // The standard layout (linux/Documentation/input/gamepad.rst)
                let bit = match code {
                    0x130 => button::A,     // South
                    0x131 => button::B,     // East
                    0x133 => button::Y,     // North
                    0x134 => button::X,     // West
                    0x136 => button::LB,
                    0x137 => button::RB,
                    0x13a => button::BACK,  // Select
                    0x13b => button::START,
                    0x13c => button::GUIDE, // Mode
                    0x13d => button::LS,
                    0x13e => button::RS,
                    0x220 => button::UP,
                    0x221 => button::DOWN,
                    0x222 => button::LEFT,
                    0x223 => button::RIGHT,
                    0x138 | 0x139 => {
                        self.trigger_buttons[(code - 0x138) as usize] = value != 0;
                        return;
                    }
                    _ => return,
                };
                if value != 0 { self.buttons |= bit } else { self.buttons &= !bit }
            }
            _ => {}
        }
    }

    /// -1–1 (a stick)
    fn stick(&self, code: u16) -> f32 {
        let (v, min, max) = self.axes[code as usize];
        if max <= min {
            return 0.0;
        }
        let mid = (min + max) as f32 / 2.0;
        ((v as f32 - mid) / ((max - min) as f32 / 2.0)).clamp(-1.0, 1.0)
    }

    /// 0–1 (a trigger)
    fn trigger(&self, code: u16) -> f32 {
        let (v, min, max) = self.axes[code as usize];
        if max <= min {
            return 0.0;
        }
        ((v - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
    }

    pub fn pad(&self) -> Pad {
        let mut buttons = self.buttons;
        // A d-pad as a hat (most drivers): -1 left/up, 1 right/down
        let hat = |code: u16| self.axes[code as usize].0.signum();
        match hat(ABS_HAT0X) {
            -1 => buttons |= button::LEFT,
            1 => buttons |= button::RIGHT,
            _ => {}
        }
        match hat(ABS_HAT0Y) {
            -1 => buttons |= button::UP,
            1 => buttons |= button::DOWN,
            _ => {}
        }
        let (right, triggers) = if self.right_on_rx { ([ABS_RX, ABS_RY], [ABS_Z, ABS_RZ]) } else { ([ABS_Z, ABS_RZ], [ABS_BRAKE, ABS_GAS]) };
        let trigger = |i: usize| if self.trigger_buttons[i] { 1.0 } else { self.trigger(triggers[i]) };
        Pad {
            buttons,
            left_trigger: trigger(0),
            right_trigger: trigger(1),
            // (Linux's y goes down; Puppet's up)
            left_stick: [self.stick(ABS_X), -self.stick(ABS_Y)],
            right_stick: [self.stick(right[0]), -self.stick(right[1])],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_windows_codes_and_presses_count_once() {
        let mut k = KeyboardMouse::default();
        k.event(EV_KEY, 17, 1); // W
        k.event(EV_KEY, 17, 2); // held: a repeat
        k.event(EV_KEY, 42, 1); // Left Shift
        let d = k.tick(0.016);
        assert!(d.down(b'W') && d.down(0xA0) && d.down(0x10), "{:?}", d.keys);
        assert_eq!(d.presses, 2);
        assert!(d.keys_idle < 0.1);
        k.event(EV_KEY, 17, 0);
        let d = k.tick(0.016);
        assert!(!d.down(b'W') && d.down(0xA0));
    }

    #[test]
    fn the_mouse_moves_and_settles_as_puppets_does() {
        let mut k = KeyboardMouse::default();
        // 800 counts right (an inch on an 800 DPI mouse), 400 down (toward you)
        k.event(EV_REL, REL_X, 800);
        k.event(EV_REL, REL_Y, 400);
        k.event(EV_KEY, BTN_LEFT, 1);
        let d = k.tick(0.0);
        assert!((d.mouse[0] - 0.0254).abs() < 1e-4, "{:?}", d.mouse);
        assert!((d.mouse[1] + 0.0127).abs() < 1e-4, "{:?}", d.mouse);
        assert_eq!(d.buttons, 1);
        assert!(d.down(0x01));
        // Still: it drifts back to the middle
        let later = (0..200).fold(d, |_, _| k.tick(0.016));
        assert!(later.mouse[0].abs() < 0.001 && later.mouse_idle > 3.0);
    }

    #[test]
    fn a_controller_reads_as_puppets_pad() {
        // An Xbox controller on xpad: sticks -32768–32767, triggers 0–1023 on Z/RZ
        let mut g = Gamepad::new(&[(ABS_X, -32768, 32767), (ABS_Y, -32768, 32767), (ABS_RX, -32768, 32767), (ABS_RY, -32768, 32767), (ABS_Z, 0, 1023), (ABS_RZ, 0, 1023), (ABS_HAT0X, -1, 1), (ABS_HAT0Y, -1, 1)]);
        let rest = g.pad();
        assert!(rest.left_stick[0].abs() < 0.01 && rest.left_trigger == 0.0 && rest.buttons == 0, "{rest:?}");
        g.event(EV_KEY, 0x130, 1); // A
        g.event(EV_ABS, ABS_Y, -32768); // left stick pushed up
        g.event(EV_ABS, ABS_RZ, 1023); // right trigger in
        g.event(EV_ABS, ABS_HAT0X, 1); // d-pad right
        let p = g.pad();
        assert_eq!(p.buttons, button::A | button::RIGHT);
        assert!((p.left_stick[1] - 1.0).abs() < 1e-3, "{p:?}");
        assert!((p.right_trigger - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_json_is_puppets() {
        // The same fields Puppet's Desk, Pad and Message read (network.rs's test sends these too)
        let mut d = Desk::default();
        d.press(b'W');
        let m = Message { desk: Some(d), pad: Some(Pad { buttons: button::A, ..Default::default() }), ..Default::default() };
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        for k in ["keys", "presses", "mouse", "buttons", "mouse_idle", "keys_idle"] {
            assert!(v["desk"].get(k).is_some(), "desk.{k}");
        }
        for k in ["buttons", "left_trigger", "right_trigger", "left_stick", "right_stick"] {
            assert!(v["pad"].get(k).is_some(), "pad.{k}");
        }
        assert!(v.get("hello").is_none());
    }
}
