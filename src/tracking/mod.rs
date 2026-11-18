//! Tracking: what moves the avatar, from any input. Each input (a phone's
//! face app, SteamVR, a Leap, MediaPipe) leaves what it sees here, already in
//! the avatar's own space (metres, Y up, facing +Z, its left at +X), with
//! the time; the avatar (motion.rs) takes the freshest of each part every
//! frame. Inputs and avatar know only this place.
//!
//! Inputs send three kinds of thing:
//! - a face: ARKit's 52 shapes, the head's turn, the eyes;
//! - a body: where the head, hands, hips and feet are (the IK's targets);
//! - hands: how far each finger joint is bent.
//!
//! The user picks which input drives which part of the avatar (Part, Pick,
//! config::Assign): expressions from the phone, arms from SteamVR, fingers
//! from the Leap… Auto takes whichever capable input sent last.

pub mod config;
mod dll;
pub mod dock;
mod fingers;
pub mod gamepad;
pub mod ifacialmocap;
pub mod leap;
pub mod mediapipe;
pub mod menu;
pub mod steamvr;
pub mod telemetry;
pub mod vts;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use glam::{Quat, Vec3};

/// ARKit's 52 blendshapes, in the order `Face::shapes` keeps them
pub const ARKIT: [&str; 52] = [
    "eyeBlinkLeft", "eyeLookDownLeft", "eyeLookInLeft", "eyeLookOutLeft", "eyeLookUpLeft", "eyeSquintLeft", "eyeWideLeft",
    "eyeBlinkRight", "eyeLookDownRight", "eyeLookInRight", "eyeLookOutRight", "eyeLookUpRight", "eyeSquintRight", "eyeWideRight",
    "jawForward", "jawLeft", "jawRight", "jawOpen",
    "mouthClose", "mouthFunnel", "mouthPucker", "mouthLeft", "mouthRight",
    "mouthSmileLeft", "mouthSmileRight", "mouthFrownLeft", "mouthFrownRight",
    "mouthDimpleLeft", "mouthDimpleRight", "mouthStretchLeft", "mouthStretchRight",
    "mouthRollLower", "mouthRollUpper", "mouthShrugLower", "mouthShrugUpper",
    "mouthPressLeft", "mouthPressRight", "mouthLowerDownLeft", "mouthLowerDownRight", "mouthUpperUpLeft", "mouthUpperUpRight",
    "browDownLeft", "browDownRight", "browInnerUp", "browOuterUpLeft", "browOuterUpRight",
    "cheekPuff", "cheekSquintLeft", "cheekSquintRight",
    "noseSneerLeft", "noseSneerRight",
    "tongueOut",
];

/// The place of an ARKit shape in `ARKIT` (any case)
pub fn arkit_index(name: &str) -> Option<usize> {
    ARKIT.iter().position(|n| n.eq_ignore_ascii_case(name))
}

/// The inputs, in the order the Tracking window lists them
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Source {
    IFacialMocap,
    MeowFace,
    SteamVr,
    Leap,
    MediaPipe,
    /// The headset's own telemetry server (as the Unity avatar app reads it)
    Telemetry,
    /// A game controller held in the hands (gamepad.rs)
    Controller,
}

impl Source {
    pub const ALL: [Source; 7] = [Source::IFacialMocap, Source::MeowFace, Source::SteamVr, Source::Telemetry, Source::Leap, Source::MediaPipe, Source::Controller];

    pub fn name(self) -> &'static str {
        match self {
            Source::IFacialMocap => "iFacialMocap",
            Source::MeowFace => "MeowFace",
            Source::SteamVr => "SteamVR",
            Source::Leap => "Leap Motion",
            Source::MediaPipe => "MediaPipe",
            Source::Telemetry => "VR telemetry",
            Source::Controller => "Controller (held)",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Face {
    /// ARKit's shapes, 0–1 (`ARKIT`'s order)
    pub shapes: [f32; 52],
    /// The head's turn from looking straight ahead, in the avatar's space
    pub head: Quat,
    /// Each eye's turn from looking straight ahead (left, right), when the
    /// input says (the shapes' eyeLook… otherwise)
    pub eyes: Option<[Quat; 2]>,
}

impl Default for Face {
    fn default() -> Self {
        Face { shapes: [0.0; 52], head: Quat::IDENTITY, eyes: None }
    }
}

/// Where something is and how it's turned, in the avatar's space
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub position: Vec3,
    pub rotation: Quat,
}

/// The IK's targets: whatever the input tracks (None: left to the idle pose)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Body {
    pub head: Option<Target>,
    pub left_hand: Option<Target>,
    pub right_hand: Option<Target>,
    pub hips: Option<Target>,
    pub left_foot: Option<Target>,
    pub right_foot: Option<Target>,
    /// Where the elbows are, to bend the arms the right way
    pub left_elbow: Option<Vec3>,
    pub right_elbow: Option<Vec3>,
}

/// One hand's fingers: thumb, index, middle, ring, little
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hand {
    /// Each finger's three joints, bent toward the palm, radians (0 = straight)
    pub bend: [[f32; 3]; 5],
    /// Each finger's sideways spread at its base, radians (0 = at rest)
    pub spread: [f32; 5],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hands {
    pub left: Option<Hand>,
    pub right: Option<Hand>,
}

struct Seen<T> {
    value: T,
    source: Source,
    at: Instant,
}

#[derive(Default)]
struct State {
    face: Vec<Seen<Face>>,
    body: Vec<Seen<Body>>,
    hands: Vec<Seen<Hands>>,
    /// Each input's last sign of life, and what it says about itself
    status: Vec<(Source, Instant, String)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// How long a part stays the avatar's after its input stops sending
pub const FRESH: Duration = Duration::from_millis(600);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(state.get_or_insert_with(State::default))
}

fn put<T>(list: &mut Vec<Seen<T>>, source: Source, value: T) {
    let seen = Seen { value, source, at: Instant::now() };
    match list.iter_mut().find(|s| s.source == source) {
        Some(slot) => *slot = seen,
        None => list.push(seen),
    }
}

pub fn set_face(source: Source, face: Face) {
    with(|s| put(&mut s.face, source, face));
}

pub fn set_body(source: Source, body: Body) {
    with(|s| put(&mut s.body, source, body));
}

pub fn set_hands(source: Source, hands: Hands) {
    with(|s| put(&mut s.hands, source, hands));
}

/// The head an input tracks now (the Leap on a headset goes by SteamVR's)
pub fn head_from(source: Source) -> Option<Target> {
    with(|s| s.body.iter().find(|b| b.source == source && b.at.elapsed() < FRESH).and_then(|b| b.value.head))
}

/// An input's line in the Tracking window ("receiving", "no device"…)
pub fn set_status(source: Source, text: impl Into<String>) {
    let text = text.into();
    with(|s| match s.status.iter_mut().find(|(src, _, _)| *src == source) {
        Some(slot) => *slot = (source, Instant::now(), text),
        None => s.status.push((source, Instant::now(), text)),
    });
}

/// What each input last said, and how long ago
pub fn statuses() -> Vec<(Source, Duration, String)> {
    with(|s| s.status.iter().map(|(src, at, text)| (*src, at.elapsed(), text.clone())).collect())
}

/// A part of the avatar an input can drive: the user picks which input
/// drives which (the Tracking window; config::Assign)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Part {
    Expressions,
    HeadTurn,
    HeadPosition,
    Arms,
    Fingers,
    LowerBody,
}

impl Part {
    pub const ALL: [Part; 6] = [Part::Expressions, Part::HeadTurn, Part::HeadPosition, Part::Arms, Part::Fingers, Part::LowerBody];

    pub fn name(self) -> &'static str {
        match self {
            Part::Expressions => "Expressions",
            Part::HeadTurn => "Head turn",
            Part::HeadPosition => "Head position",
            Part::Arms => "Arms",
            Part::Fingers => "Fingers",
            Part::LowerBody => "Hips and feet",
        }
    }

    /// The inputs that can drive it
    pub fn inputs(self) -> &'static [Source] {
        match self {
            Part::Expressions => &[Source::IFacialMocap, Source::MeowFace, Source::MediaPipe],
            Part::HeadTurn => &[Source::IFacialMocap, Source::MeowFace, Source::MediaPipe, Source::SteamVr, Source::Telemetry],
            Part::HeadPosition => &[Source::SteamVr, Source::Telemetry, Source::IFacialMocap, Source::MediaPipe],
            Part::Arms => &[Source::SteamVr, Source::Telemetry, Source::Leap, Source::MediaPipe, Source::Controller],
            Part::Fingers => &[Source::Leap, Source::MediaPipe, Source::SteamVr, Source::Telemetry],
            Part::LowerBody => &[Source::SteamVr, Source::Telemetry, Source::MediaPipe],
        }
    }
}

/// Which input drives a part
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Pick {
    /// Whichever of its inputs sent last
    #[default]
    Auto,
    /// Nothing: the idle pose keeps it
    Off,
    Only(Source),
}

/// What the avatar goes by this frame: each part from the input picked for
/// it (None: nothing fresh, or Off)
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub shapes: Option<[f32; 52]>,
    pub eyes: Option<[Quat; 2]>,
    pub head_turn: Option<Quat>,
    pub head_position: Option<Vec3>,
    pub left_hand: Option<Target>,
    pub right_hand: Option<Target>,
    pub left_elbow: Option<Vec3>,
    pub right_elbow: Option<Vec3>,
    pub left_fingers: Option<Hand>,
    pub right_fingers: Option<Hand>,
    pub hips: Option<Target>,
    pub left_foot: Option<Target>,
    pub right_foot: Option<Target>,
    /// Which input each part came from (the Tracking window says)
    pub from: Vec<(Part, Source)>,
    /// The controller held (Arms set to it): the hands go on it, the fingers press it
    pub controller: Option<gamepad::Pad>,
}

/// The newest fresh thing from an input `part` may use, that has `get`:
/// (when, from which input, what)
fn pick<T, V>(list: &[Seen<T>], part: Part, how: Pick, get: impl Fn(&T) -> Option<V>) -> Option<(Instant, Source, V)> {
    if how == Pick::Off {
        return None;
    }
    list.iter()
        .filter(|s| s.at.elapsed() < FRESH && part.inputs().contains(&s.source))
        .filter(|s| match how {
            Pick::Only(source) => s.source == source,
            _ => true,
        })
        .filter_map(|s| Some((s.at, s.source, get(&s.value)?)))
        .max_by_key(|(at, _, _)| *at)
}

/// Each part from the input picked for it in `assign`
pub fn snapshot(assign: &config::Assign) -> Snapshot {
    with(|s| {
        let mut out = Snapshot::default();
        fn note(from: &mut Vec<(Part, Source)>, part: Part, source: Source) {
            if !from.iter().any(|(p, _)| *p == part) {
                from.push((part, source));
            }
        }
        if let Some((_, src, f)) = pick(&s.face, Part::Expressions, assign.expressions, |f| Some(f.clone())) {
            out.shapes = Some(f.shapes);
            out.eyes = f.eyes;
            note(&mut out.from, Part::Expressions, src);
        }
        // The head's turn: a face app's, or a headset's, whichever is newer
        let from_face = pick(&s.face, Part::HeadTurn, assign.head_turn, |f| Some(f.head));
        let from_body = pick(&s.body, Part::HeadTurn, assign.head_turn, |b| b.head.map(|h| h.rotation));
        let turn = match (from_face, from_body) {
            (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
            (a, b) => a.or(b),
        };
        if let Some((_, src, q)) = turn {
            out.head_turn = Some(q);
            note(&mut out.from, Part::HeadTurn, src);
        }
        if let Some((_, src, p)) = pick(&s.body, Part::HeadPosition, assign.head_position, |b| b.head.map(|h| h.position)) {
            out.head_position = Some(p);
            note(&mut out.from, Part::HeadPosition, src);
        }
        // Each arm on its own: a Leap may see one hand only
        if let Some((_, src, t)) = pick(&s.body, Part::Arms, assign.arms, |b| b.left_hand) {
            out.left_hand = Some(t);
            out.left_elbow = s.body.iter().find(|b| b.source == src).and_then(|b| b.value.left_elbow);
            note(&mut out.from, Part::Arms, src);
        }
        if let Some((_, src, t)) = pick(&s.body, Part::Arms, assign.arms, |b| b.right_hand) {
            out.right_hand = Some(t);
            out.right_elbow = s.body.iter().find(|b| b.source == src).and_then(|b| b.value.right_elbow);
            note(&mut out.from, Part::Arms, src);
        }
        if let Some((_, src, h)) = pick(&s.hands, Part::Fingers, assign.fingers, |h| h.left) {
            out.left_fingers = Some(h);
            note(&mut out.from, Part::Fingers, src);
        }
        if let Some((_, src, h)) = pick(&s.hands, Part::Fingers, assign.fingers, |h| h.right) {
            out.right_fingers = Some(h);
            note(&mut out.from, Part::Fingers, src);
        }
        if let Some((_, src, t)) = pick(&s.body, Part::LowerBody, assign.lower_body, |b| b.hips) {
            out.hips = Some(t);
            note(&mut out.from, Part::LowerBody, src);
        }
        if let Some((_, src, t)) = pick(&s.body, Part::LowerBody, assign.lower_body, |b| b.left_foot) {
            out.left_foot = Some(t);
            note(&mut out.from, Part::LowerBody, src);
        }
        if let Some((_, src, t)) = pick(&s.body, Part::LowerBody, assign.lower_body, |b| b.right_foot) {
            out.right_foot = Some(t);
            note(&mut out.from, Part::LowerBody, src);
        }
        // Arms on the controller: the hands hold it (and its fingers are the controller's)
        if assign.arms == Pick::Only(Source::Controller) {
            out.controller = gamepad::latest();
            if out.controller.is_some() {
                note(&mut out.from, Part::Arms, Source::Controller);
                note(&mut out.from, Part::Fingers, Source::Controller);
            }
        }
        out
    })
}

/// Forgets everything seen (the tests; an input turned off forgets itself by going stale)
pub fn clear() {
    with(|s| *s = State::default());
}

/// A turn given as Unity does (degrees about X, Y, Z, applied Z then X then
/// Y, left-handed, X to the avatar's right) in the avatar's own space
/// (right-handed, X to its left): what the phone apps send.
pub fn from_unity_euler(x: f32, y: f32, z: f32) -> Quat {
    let q = Quat::from_rotation_y(y.to_radians()) * Quat::from_rotation_x(x.to_radians()) * Quat::from_rotation_z(z.to_radians());
    // Mirrored across X: the turn about X stays, those about Y and Z reverse
    Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
}

/// Starts (or restarts, after its settings changed) every enabled input.
pub fn start() {
    let cfg = config::get();
    ifacialmocap::restart(&cfg);
    vts::restart(&cfg);
    steamvr::restart(&cfg);
    telemetry::restart(&cfg);
    leap::restart(&cfg);
    mediapipe::restart(&cfg);
    gamepad::restart(&cfg.controller);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arkit_names_are_found() {
        assert_eq!(arkit_index("jawOpen"), Some(17));
        assert_eq!(arkit_index("EyeBlinkLeft"), Some(0));
        assert_eq!(arkit_index("tongueOut"), Some(51));
        assert_eq!(arkit_index("nope"), None);
    }

    #[test]
    fn a_unity_turn_becomes_the_avatars() {
        // Nodding down (Unity: +X) tips the forward axis down
        let down = from_unity_euler(30.0, 0.0, 0.0) * Vec3::Z;
        assert!(down.y < -0.4 && down.z > 0.8, "{down:?}");
        // Turning to the avatar's right (Unity: +Y) is toward -X here
        let right = from_unity_euler(0.0, 30.0, 0.0) * Vec3::Z;
        assert!(right.x < -0.4 && right.z > 0.8, "{right:?}");
    }
}
