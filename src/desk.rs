//! The avatar at a desk, its Arms on the Keyboard & mouse input
//! (tracking/desk.rs): where its hands go on the keyboard and mouse
//! (controller/desk.glb, or the source's own GLB), what its fingers do, which
//! keys sink. controller.rs draws it, as it draws the gamepad.
//!
//! What the keys show is the user's choice, since viewers could read typing:
//! - Exact: the finger goes to the key pressed, and that key sinks;
//! - Area: WASD, Space, Shift and Ctrl show exactly (games); any other key
//!   only makes the hand on its side tap (a finger of no meaning);
//! - Motion: every key only makes a hand tap.
//!
//! The hands: left on the keys and right on the mouse (gaming; mirrored for a
//! left-handed user), both typing and one going to the mouse while it's used,
//! or both typing. Local space is the GLB's: +X the user's left, +Y up, +Z
//! away from the user, metres.

use glam::Vec3;

use crate::model::Model;
use crate::pose::Pose;
use crate::tracking::Hand;
use crate::tracking::desk::Desk;

/// A key's pitch
const U: f32 = 0.019;

/// The keys the built-in GLB has: (virtual-key code, its node's name after key_)
pub const KEYS: &[(u8, &str)] = &[
    (0x1B, "esc"), (0x31, "1"), (0x32, "2"), (0x33, "3"), (0x34, "4"), (0x35, "5"), (0x36, "6"), (0x37, "7"), (0x38, "8"), (0x39, "9"), (0x30, "0"),
    (0xBD, "minus"), (0xBB, "equals"), (0x08, "backspace"),
    (0x09, "tab"), (0x51, "q"), (0x57, "w"), (0x45, "e"), (0x52, "r"), (0x54, "t"), (0x59, "y"), (0x55, "u"), (0x49, "i"), (0x4F, "o"), (0x50, "p"),
    (0xDB, "lbracket"), (0xDD, "rbracket"), (0xDC, "backslash"),
    (0x14, "caps"), (0x41, "a"), (0x53, "s"), (0x44, "d"), (0x46, "f"), (0x47, "g"), (0x48, "h"), (0x4A, "j"), (0x4B, "k"), (0x4C, "l"),
    (0xBA, "semicolon"), (0xDE, "quote"), (0x0D, "enter"),
    (0xA0, "lshift"), (0x5A, "z"), (0x58, "x"), (0x43, "c"), (0x56, "v"), (0x42, "b"), (0x4E, "n"), (0x4D, "m"),
    (0xBC, "comma"), (0xBE, "period"), (0xBF, "slash"), (0xA1, "rshift"),
    (0xA2, "lctrl"), (0x5B, "lwin"), (0xA4, "lalt"), (0x20, "space"), (0xA5, "ralt"), (0xA3, "rctrl"),
    (0x25, "left"), (0x26, "up"), (0x27, "right"), (0x28, "down"),
];

/// Shown exactly in Area mode (what games use)
const GAME_KEYS: &[&str] = &["w", "a", "s", "d", "space", "lshift", "lctrl"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyDetail {
    #[default]
    Area,
    Exact,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HandMode {
    /// One hand on the keys, the other on the mouse
    #[default]
    Gaming,
    /// Both typing; the mouse hand goes to the mouse while it's used
    Switching,
    /// Both on the keys
    Typing,
}

/// The source's Keyboard & mouse settings
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    pub detail: KeyDetail,
    pub hands: HandMode,
    /// The mouse on the left, the right hand on the keys (the arrows, gaming)
    pub left_handed: bool,
    /// Each hand (left, right) raised (+) or lowered (-), and moved forward
    /// (+, away from the user) or back (-), metres
    pub hand_up: [f32; 2],
    pub hand_forward: [f32; 2],
    /// The hands tilted forward (+, fingertips down) or back (-), degrees
    pub hand_tilt: f32,
    /// The fingers curled more (+) or less (-), radians at each joint
    pub curl: f32,
    /// On the mouse, the index and middle fingers lowered (+) or raised (-)
    /// at the knuckle, degrees
    pub mouse_fingers: f32,
}

#[derive(Clone, Debug)]
struct Key {
    vk: u8,
    name: &'static str,
    node: usize,
    /// Its top, at rest
    at: Vec3,
}

/// The keyboard and mouse a GLB has
#[derive(Clone, Debug)]
pub struct Layout {
    keys: Vec<Key>,
    /// The keyboard's body and keys (they move together for a left-handed user)
    keyboard: Vec<usize>,
    /// The mouse and its buttons (they move with the mouse)
    mouse: Vec<usize>,
    mouse_at: Vec3,
    buttons: [Option<usize>; 2],
    /// The keyboard's middle (x)
    middle: f32,
}

impl Layout {
    /// The desk in `model` (rest: its rest pose); None when it has no keyboard
    pub fn find(model: &Model, rest: &Pose) -> Option<Layout> {
        let node = |n: &str| model.nodes.iter().position(|x| x.name == n);
        let board = node("keyboard")?;
        let keys: Vec<Key> = KEYS.iter().filter_map(|&(vk, name)| node(&format!("key_{name}")).map(|n| Key { vk, name, node: n, at: rest.position(n) })).collect();
        let mouse_body = node("mouse");
        let buttons = [node("mouse_left"), node("mouse_right")];
        let mouse = mouse_body.into_iter().chain(buttons.iter().flatten().copied()).collect();
        Some(Layout {
            keyboard: std::iter::once(board).chain(keys.iter().map(|k| k.node)).collect(),
            middle: rest.position(board).x,
            mouse_at: mouse_body.map(|m| rest.position(m)).unwrap_or(Vec3::new(-0.26, 0.02, 0.0)),
            keys,
            mouse,
            buttons,
        })
    }

    fn key(&self, name: &str) -> Option<&Key> {
        self.keys.iter().find(|k| k.name == name)
    }

    /// How far the keyboard moves sideways (a left-handed user has it on the right)
    fn board_shift(&self, look: &Look) -> f32 {
        if look.left_handed { -2.0 * self.middle } else { 0.0 }
    }

    /// A key's top where it is now
    fn key_at(&self, key: &Key, look: &Look) -> Vec3 {
        key.at + Vec3::X * self.board_shift(look)
    }

    /// The mouse where it is now: mirrored for a left-handed user, moved
    /// with the real one (to the user's right is -X, away is +Z)
    fn mouse_now(&self, look: &Look, input: &Desk) -> Vec3 {
        let rest = if look.left_handed { Vec3::new(-self.mouse_at.x, self.mouse_at.y, self.mouse_at.z) } else { self.mouse_at };
        rest + Vec3::new(-input.mouse[0], 0.0, input.mouse[1])
    }
}

/// A finger tapping (Area and Motion modes): which hand, which finger, how long left
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tap {
    hand: usize,
    finger: usize,
    left: f32,
}

const TAP: f32 = 0.14;

/// What changes over time
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The mouse hand on the mouse (1) or the keys (0), easing between
    on_mouse: f32,
    /// The keys down last frame (new presses tap)
    keys: [u64; 4],
    taps: Vec<Tap>,
    /// The wrists moved toward the keys pressed (eased)
    shift: [Vec3; 2],
    count: u32,
}

/// The hand on the mouse (0 left, 1 right)
fn mouse_hand(look: &Look) -> usize {
    if look.left_handed { 0 } else { 1 }
}

/// The hands on the keys now
fn key_hands(look: &Look, state: &State) -> Vec<usize> {
    match look.hands {
        HandMode::Gaming => vec![1 - mouse_hand(look)],
        HandMode::Typing => vec![0, 1],
        HandMode::Switching => {
            if state.on_mouse < 0.5 { vec![0, 1] } else { vec![1 - mouse_hand(look)] }
        }
    }
}

/// Where a hand's index finger rests (its home key), on the keys
fn home<'a>(layout: &'a Layout, look: &Look, hand: usize) -> Option<&'a Key> {
    let gaming = look.hands == HandMode::Gaming;
    let name = match (hand, gaming) {
        (0, true) => "d",
        (1, true) => "left",
        (0, false) => "f",
        _ => "j",
    };
    layout.key(name).or_else(|| layout.keys.first())
}

/// The finger columns of a hand whose index is at `x` (left hand: the others
/// toward +X, its left), finger 1 index … 4 little
fn column(x: f32, hand: usize, finger: usize) -> f32 {
    let s = if hand == 0 { 1.0 } else { -1.0 };
    x + s * (finger as f32 - 1.0) * U
}

/// Which hand presses `key` (of those on the keys)
fn hand_for(layout: &Layout, look: &Look, hands: &[usize], key: &Key) -> usize {
    if hands.len() == 1 {
        return hands[0];
    }
    let (l, r) = (home(layout, look, 0).map(|k| layout.key_at(k, look).x), home(layout, look, 1).map(|k| layout.key_at(k, look).x));
    let split = match (l, r) {
        (Some(l), Some(r)) => (l + r) / 2.0,
        _ => layout.middle,
    };
    if layout.key_at(key, look).x > split { 0 } else { 1 }
}

/// The finger for a key, from the hand's columns (the space bar: the thumb)
fn finger_for(layout: &Layout, look: &Look, hand: usize, key: &Key) -> usize {
    if key.name == "space" {
        return 0;
    }
    let Some(h) = home(layout, look, hand) else { return 1 };
    let hx = layout.key_at(h, look).x;
    let kx = layout.key_at(key, look).x;
    (1..=4).min_by(|&a, &b| (kx - column(hx, hand, a)).abs().total_cmp(&(kx - column(hx, hand, b)).abs())).unwrap_or(1)
}

/// Shown exactly (the finger goes to it and it sinks)
fn exact(look: &Look, key: &Key) -> bool {
    match look.detail {
        KeyDetail::Exact => true,
        KeyDetail::Area => GAME_KEYS.contains(&key.name),
        KeyDetail::Motion => false,
    }
}

/// `dt` seconds later
pub fn step(layout: &Layout, state: &mut State, look: &Look, input: &Desk, dt: f32) {
    // The mouse hand: on the mouse for gaming, while it's used for switching
    let target = match look.hands {
        HandMode::Gaming => 1.0,
        HandMode::Typing => 0.0,
        HandMode::Switching => {
            if input.mouse_idle < 1.2 { 1.0 } else { 0.0 }
        }
    };
    let k = (dt / 0.25).min(1.0);
    state.on_mouse += (target - state.on_mouse).clamp(-k, k);

    // New presses: a tap for each key that doesn't show exactly
    let hands = key_hands(look, state);
    for k in &layout.keys {
        let new = input.down(k.vk) && state.keys[(k.vk >> 6) as usize] & (1 << (k.vk & 63)) == 0;
        if new && !exact(look, k) && !hands.is_empty() {
            state.count = state.count.wrapping_add(1);
            // Area: the hand on its side; Motion: the hands in turn
            let hand = if look.detail == KeyDetail::Area { hand_for(layout, look, &hands, k) } else { hands[state.count as usize % hands.len()] };
            // A finger of no meaning (the same few in turn)
            let finger = 1 + (state.count.wrapping_mul(2_654_435_761) >> 16) as usize % 4;
            state.taps.retain(|t| t.hand != hand);
            state.taps.push(Tap { hand, finger, left: TAP });
        }
    }
    state.keys = input.keys;
    for t in &mut state.taps {
        t.left -= dt;
    }
    state.taps.retain(|t| t.left > 0.0);

    // The wrists toward the keys pressed (exactly shown ones only)
    let mut want = [Vec3::ZERO; 2];
    let mut count = [0.0f32; 2];
    for k in layout.keys.iter().filter(|k| input.down(k.vk) && exact(look, k)) {
        let hand = hand_for(layout, look, &hands, k);
        let Some(h) = home(layout, look, hand) else { continue };
        let finger = finger_for(layout, look, hand, k);
        let (kp, hp) = (layout.key_at(k, look), layout.key_at(h, look));
        // Sideways only what the finger can't reach, toward the row by most of it
        let reach = if finger == 0 { 0.0 } else { kp.x - column(hp.x, hand, finger) };
        let dx = reach.signum() * (reach.abs() - 0.5 * U).max(0.0);
        let dz = if finger == 0 { 0.0 } else { (kp.z - hp.z) * 0.75 };
        want[hand] += Vec3::new(dx, 0.0, dz);
        count[hand] += 1.0;
    }
    let k = (dt * 14.0).min(1.0);
    for h in 0..2 {
        let target = if count[h] > 0.0 { want[h] / count[h] } else { Vec3::ZERO };
        state.shift[h] += (target - state.shift[h]) * k;
    }
}

/// A hand's wrist: where it goes, the fingers' direction and the palm's (local)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wrist {
    pub at: Vec3,
    pub fingers: Vec3,
    pub palm: Vec3,
}

/// Both wrists now
pub fn wrists(layout: &Layout, state: &State, look: &Look, input: &Desk) -> [Wrist; 2] {
    let mh = mouse_hand(look);
    // The tilt slider: about the hands' side-to-side axis
    let tilt = glam::Quat::from_rotation_x(look.hand_tilt.to_radians());
    [0, 1].map(|hand| {
        let s = if hand == 0 { 1.0 } else { -1.0 };
        let on_keys = home(layout, look, hand).map(|h| {
            let hp = layout.key_at(h, look);
            Wrist {
                // (Seen live: 4 cm over the keys was too low)
                at: hp + Vec3::new(s * 0.5 * U, 0.07, -0.10) + state.shift[hand],
                fingers: Vec3::new(-s * 0.12, -0.35, 1.0),
                palm: Vec3::new(0.0, -1.0, 0.15),
            }
        });
        let on_mouse = Wrist { at: layout.mouse_now(look, input) + Vec3::new(0.0, 0.065, -0.10), fingers: Vec3::new(0.0, -0.25, 1.0), palm: Vec3::new(0.0, -1.0, 0.1) };
        let keys = on_keys.unwrap_or(on_mouse);
        let t = if hand == mh { state.on_mouse } else { 0.0 };
        let w = if t <= 0.0 {
            keys
        } else if t >= 1.0 {
            on_mouse
        } else {
            // Lifted on the way over
            let lift = Vec3::Y * 0.05 * (t * std::f32::consts::PI).sin();
            Wrist { at: keys.at.lerp(on_mouse.at, t) + lift, fingers: keys.fingers.lerp(on_mouse.fingers, t), palm: keys.palm.lerp(on_mouse.palm, t) }
        };
        // This hand's own sliders
        let own = Vec3::new(0.0, look.hand_up[hand], look.hand_forward[hand]);
        Wrist { at: w.at + own, fingers: tilt * w.fingers, palm: tilt * w.palm }
    })
}

// (Seen live: 0.5 / 0.65 / 0.4 looked too straight)
const REST: [f32; 3] = [0.75, 0.9, 0.6];
const PRESS: [f32; 3] = [1.0, 1.05, 0.7];

/// Both hands' fingers now (left, right)
pub fn fingers(layout: &Layout, state: &State, look: &Look, input: &Desk) -> [Hand; 2] {
    let mh = mouse_hand(look);
    let hands = key_hands(look, state);
    let typing = |hand: usize| {
        let mut h = Hand::default();
        for f in 1..5 {
            h.bend[f] = REST;
        }
        h.bend[0] = [0.3, 0.25, 0.15];
        // Pressing: the key's finger down, exactly shown keys only
        for k in layout.keys.iter().filter(|k| input.down(k.vk) && exact(look, k)) {
            if hand_for(layout, look, &hands, k) == hand {
                let f = finger_for(layout, look, hand, k);
                h.bend[f] = if f == 0 { [0.45, 0.35, 0.2] } else { PRESS };
            }
        }
        for t in state.taps.iter().filter(|t| t.hand == hand) {
            // Down and up again over the tap
            let a = (t.left / TAP * std::f32::consts::PI).sin();
            h.bend[t.finger] = [0, 1, 2].map(|j| REST[j] + (PRESS[j] - REST[j]) * a);
        }
        h
    };
    let mousing = || {
        let mut h = Hand::default();
        let click = |b: u8| input.buttons & b != 0;
        // (Seen live: 0.45 / 0.6 at the knuckle sat above the keyboard hand's;
        // the rest of the gap was the whole hand's pitch, see wrists)
        h.bend[1] = if click(1) { [0.82, 0.7, 0.5] } else { [0.67, 0.55, 0.4] };
        h.bend[2] = if click(2) { [0.82, 0.7, 0.5] } else { [0.67, 0.55, 0.4] };
        // The mouse fingers slider
        for f in 1..3 {
            h.bend[f][0] += look.mouse_fingers.to_radians();
        }
        h.bend[3] = [0.85, 0.95, 0.65];
        h.bend[4] = [0.9, 1.0, 0.65];
        h.bend[0] = [0.15, 0.1, 0.05];
        h.spread[0] = -0.3;
        h
    };
    [0, 1].map(|hand| {
        let t = if hand == mh { state.on_mouse } else { 0.0 };
        let (a, b) = (typing(hand), mousing());
        let mut h = if t <= 0.0 {
            a
        } else if t >= 1.0 {
            b
        } else {
            crate::motion::mix_hand(a, b, t)
        };
        // The curl slider, on the four fingers
        for f in 1..5 {
            h.bend[f] = h.bend[f].map(|v| (v + look.curl).max(0.0));
        }
        h
    })
}

/// What moves on the desk (local): (node, its translation from rest)
pub fn moved(layout: &Layout, look: &Look, input: &Desk) -> Vec<(usize, Vec3)> {
    let mut out = Vec::new();
    let shift = Vec3::X * layout.board_shift(look);
    for &n in &layout.keyboard {
        let sunk = layout.keys.iter().any(|k| k.node == n && input.down(k.vk) && exact(look, k));
        out.push((n, shift - if sunk { Vec3::Y * 0.003 } else { Vec3::ZERO }));
    }
    let mouse = layout.mouse_now(look, input) - layout.mouse_at;
    for &n in &layout.mouse {
        let click = layout.buttons.iter().position(|b| *b == Some(n)).is_some_and(|i| input.buttons & (1 << i) != 0);
        out.push((n, mouse - if click { Vec3::Y * 0.0015 } else { Vec3::ZERO }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn desk() -> Option<(Arc<Model>, Layout)> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("controller").join("desk.glb");
        let model = Arc::new(crate::model::load(&path).ok()?);
        let rest = Pose::rest(&model);
        let layout = Layout::find(&model, &rest)?;
        Some((model, layout))
    }

    fn vk(name: &str) -> u8 {
        KEYS.iter().find(|(_, n)| *n == name).unwrap().0
    }

    fn pressed(names: &[&str]) -> Desk {
        let mut d = Desk { mouse_idle: 10.0, keys_idle: 0.0, ..Default::default() };
        for n in names {
            d.press(vk(n));
        }
        d
    }

    #[test]
    fn the_built_in_desk_has_every_key() {
        let Some((_, layout)) = desk() else { return };
        assert_eq!(layout.keys.len(), KEYS.len());
        assert_eq!(layout.mouse.len(), 3);
        // The user's left is +X, away is +Z: W is left of D and farther
        let (w, d) = (layout.key("w").unwrap().at, layout.key("d").unwrap().at);
        assert!(w.x > d.x && w.z > d.z);
        assert!(layout.mouse_at.x < layout.key("right").unwrap().at.x);
    }

    #[test]
    fn wasd_fingers_and_typing_fingers() {
        let Some((_, layout)) = desk() else { return };
        let gaming = Look::default();
        let key = |n: &str| layout.key(n).unwrap();
        // Gaming, left hand on WASD: D index, S middle (W too), A ring, shift little, space thumb
        assert_eq!(finger_for(&layout, &gaming, 0, key("d")), 1);
        assert_eq!(finger_for(&layout, &gaming, 0, key("s")), 2);
        assert_eq!(finger_for(&layout, &gaming, 0, key("w")), 2);
        assert_eq!(finger_for(&layout, &gaming, 0, key("a")), 3);
        assert_eq!(finger_for(&layout, &gaming, 0, key("lshift")), 4);
        assert_eq!(finger_for(&layout, &gaming, 0, key("space")), 0);
        // Typing: K is the right hand's middle finger
        let typing = Look { hands: HandMode::Typing, detail: KeyDetail::Exact, ..Default::default() };
        assert_eq!(hand_for(&layout, &typing, &[0, 1], key("k")), 1);
        assert_eq!(finger_for(&layout, &typing, 1, key("k")), 2);
        assert_eq!(hand_for(&layout, &typing, &[0, 1], key("e")), 0);
    }

    #[test]
    fn what_the_keys_show_is_the_users_choice() {
        let Some((_, layout)) = desk() else { return };
        let input = pressed(&["w", "h"]);
        let sunk = |look: Look| -> Vec<&str> {
            moved(&layout, &look, &input)
                .iter()
                .filter(|(_, t)| t.y < 0.0)
                .filter_map(|(n, _)| layout.keys.iter().find(|k| k.node == *n).map(|k| k.name))
                .collect()
        };
        // Exact: both sink; Area: only the game key; Motion: none
        let mut exact = sunk(Look { detail: KeyDetail::Exact, ..Default::default() });
        exact.sort();
        assert_eq!(exact, ["h", "w"]);
        assert_eq!(sunk(Look::default()), ["w"]);
        assert!(sunk(Look { detail: KeyDetail::Motion, ..Default::default() }).is_empty());

        // Area, typing: H only taps a finger of the right hand, whichever key it is
        let look = Look { hands: HandMode::Typing, ..Default::default() };
        let mut state = State::default();
        step(&layout, &mut state, &look, &pressed(&["h"]), 0.01);
        assert_eq!(state.taps.len(), 1);
        assert_eq!(state.taps[0].hand, 1);
        // The same press of another key on that side: the same kind of tap, no key-specific finger
        let mut other = State::default();
        step(&layout, &mut other, &look, &pressed(&["l"]), 0.01);
        assert_eq!(other.taps[0].finger, state.taps[0].finger, "the finger follows the count, not the key");
    }

    #[test]
    fn the_mouse_hand_goes_to_the_mouse() {
        let Some((_, layout)) = desk() else { return };
        let mut state = State::default();
        let gaming = Look::default();
        let still = pressed(&[]);
        for _ in 0..30 {
            step(&layout, &mut state, &gaming, &still, 0.016);
        }
        let [l, r] = wrists(&layout, &state, &gaming, &still);
        // Gaming: the left hand by WASD, the right one at the mouse (to its right)
        assert!(l.at.x > layout.middle && r.at.x < layout.key("right").unwrap().at.x, "{l:?} {r:?}");
        // Moving the mouse moves the hand on it
        let moved_input = Desk { mouse: [0.03, 0.0], ..still };
        let r2 = wrists(&layout, &state, &gaming, &moved_input)[1];
        assert!((r.at.x - r2.at.x - 0.03).abs() < 1e-5);
        // Left-handed: mirrored
        let lefty = Look { left_handed: true, ..Default::default() };
        let [l, r] = wrists(&layout, &state, &lefty, &still);
        assert!(l.at.x > 0.15 && r.at.x < l.at.x, "{l:?} {r:?}");
        // Switching: both typing until the mouse is used
        let switching = Look { hands: HandMode::Switching, ..Default::default() };
        let mut state = State::default();
        for _ in 0..30 {
            step(&layout, &mut state, &switching, &still, 0.016);
        }
        let typing_r = wrists(&layout, &state, &switching, &still)[1];
        let used = Desk { mouse_idle: 0.0, ..still };
        for _ in 0..30 {
            step(&layout, &mut state, &switching, &used, 0.016);
        }
        let mousing_r = wrists(&layout, &state, &switching, &used)[1];
        assert!(mousing_r.at.x < typing_r.at.x - 0.1, "{typing_r:?} → {mousing_r:?}");
    }

    #[test]
    fn pressed_keys_bend_their_finger() {
        let Some((_, layout)) = desk() else { return };
        let look = Look::default();
        // Gaming: the right hand settles on the mouse first
        let mut state = State::default();
        for _ in 0..30 {
            step(&layout, &mut state, &look, &pressed(&[]), 0.016);
        }
        let rest = fingers(&layout, &state, &look, &pressed(&[]));
        let w = fingers(&layout, &state, &look, &pressed(&["w"]));
        // W: the left middle finger down; the right hand (mouse) unchanged
        assert!(w[0].bend[2][0] > rest[0].bend[2][0]);
        assert_eq!(w[1], rest[1]);
        let click = fingers(&layout, &state, &look, &Desk { buttons: 1, ..pressed(&[]) });
        assert!(click[1].bend[1][0] > rest[1].bend[1][0]);
    }

    #[test]
    fn the_hands_tilt_and_the_fingers_curl() {
        let Some((_, layout)) = desk() else { return };
        let state = State::default();
        let still = pressed(&[]);
        let flat = wrists(&layout, &state, &Look::default(), &still);
        let forward = wrists(&layout, &state, &Look { hand_tilt: 20.0, ..Default::default() }, &still);
        // Forward: the fingertips point lower, the wrist stays where it was
        for h in 0..2 {
            assert!(forward[h].fingers.normalize().y < flat[h].fingers.normalize().y - 0.2);
            assert_eq!(forward[h].at, flat[h].at);
        }
        // Each hand's own height and distance, the other left alone
        let moved = wrists(&layout, &state, &Look { hand_up: [0.02, 0.0], hand_forward: [0.0, -0.03], ..Default::default() }, &still);
        assert!((moved[0].at - flat[0].at - Vec3::new(0.0, 0.02, 0.0)).length() < 1e-6);
        assert!((moved[1].at - flat[1].at - Vec3::new(0.0, 0.0, -0.03)).length() < 1e-6);
        let back = wrists(&layout, &state, &Look { hand_tilt: -20.0, ..Default::default() }, &still);
        assert!(back[0].fingers.normalize().y > flat[0].fingers.normalize().y);
        // Curl: every joint of the four fingers bends more, the thumb doesn't
        let rest = fingers(&layout, &state, &Look::default(), &still);
        let curled = fingers(&layout, &state, &Look { curl: 0.3, ..Default::default() }, &still);
        for f in 1..5 {
            assert!((curled[0].bend[f][1] - rest[0].bend[f][1] - 0.3).abs() < 1e-6);
        }
        assert_eq!(curled[0].bend[0], rest[0].bend[0]);
    }
}
