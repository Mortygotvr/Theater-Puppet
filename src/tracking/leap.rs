//! Leap Motion (Ultraleap): the hands, through the Leap software's own
//! library (LeapC.dll, found where the software is installed) talking to its
//! service.
//!
//! The Leap gives millimetres from the device. On the desk, looking up: X to
//! your right, Y up, Z toward you. On a headset, looking forward: the device
//! is turned, so its Y is your forward and its Z your down.

// LeapC's structures are kept whole, used or not
#![allow(dead_code)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::Vec3;

use super::dll::Library;
use super::{Body, Hand, Hands, Source, Target, config::Config};

// LeapC.h's structures (packed to 1 byte there)

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LeapVector {
    x: f32,
    y: f32,
    z: f32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LeapBone {
    prev_joint: LeapVector,
    next_joint: LeapVector,
    width: f32,
    rotation: [f32; 4],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LeapDigit {
    finger_id: i32,
    /// Metacarpal, proximal, intermediate, distal
    bones: [LeapBone; 4],
    is_extended: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LeapPalm {
    position: LeapVector,
    stabilized_position: LeapVector,
    velocity: LeapVector,
    normal: LeapVector,
    width: f32,
    direction: LeapVector,
    orientation: [f32; 4],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LeapHand {
    id: u32,
    flags: u32,
    /// 0 left, 1 right
    kind: i32,
    confidence: f32,
    visible_time: u64,
    pinch_distance: f32,
    grab_angle: f32,
    pinch_strength: f32,
    grab_strength: f32,
    palm: LeapPalm,
    /// Thumb, index, middle, ring, little
    digits: [LeapDigit; 5],
    arm: LeapBone,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LeapTrackingEvent {
    reserved: *const c_void,
    frame_id: i64,
    timestamp: i64,
    tracking_frame_id: i64,
    hands: u32,
    hand: *const LeapHand,
    framerate: f32,
}

/// Room for LEAP_CONNECTION_MESSAGE in any version of the library
#[repr(C, packed)]
struct LeapMessage {
    size: u32,
    kind: i32,
    event: *const c_void,
    device_id: u32,
    spare: [u8; 44],
}

const EVENT_TRACKING: i32 = 0x100;
const POLICY_OPTIMIZE_HMD: u64 = 0x4;

type Create = unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> i32;
type Open = unsafe extern "C" fn(*mut c_void) -> i32;
type Poll = unsafe extern "C" fn(*mut c_void, u32, *mut LeapMessage) -> i32;
type Policy = unsafe extern "C" fn(*mut c_void, u64, u64) -> i32;
type Close = unsafe extern "C" fn(*mut c_void);

/// Where the Leap software keeps its library
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(bin) = crate::data_dir().and_then(|d| Some(d.parent()?.join("bin").join("64bit"))) {
        out.push(bin.join("LeapC.dll"));
    }
    let root = PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into()));
    out.push(root.join(r"Ultraleap\LeapSDK\lib\x64\LeapC.dll"));
    out.push(root.join(r"Ultraleap\TrackingService\bin\LeapC.dll"));
    out.push(root.join(r"Ultraleap\OpenXR\LeapC.dll"));
    out.push(root.join(r"Leap Motion\Core Services\LeapC.dll"));
    out.push(root.join(r"Leap Motion\Core Services\VRVisualizer\VRVisualizer_Data\Plugins\LeapC.dll"));
    out.push(root.join(r"Leap Motion\Core Services\Visualizer\Visualizer_Data\Plugins\LeapC.dll"));
    out
}

/// A direction from the Leap's space to the avatar's (Y up, facing +Z, its
/// left at +X)
fn turned(v: LeapVector, on_head: bool) -> Vec3 {
    if on_head {
        // Device X: your left; Y: your forward; Z: your down
        Vec3::new(v.x, -v.z, v.y)
    } else {
        // Device X: your right; Z: toward you
        Vec3::new(-v.x, v.y, -v.z)
    }
}

/// Where the device is and how it's turned, for a user `height` tall
/// standing at the avatar's place: on a desk in front of the chest, or on
/// the headset (`head`: the headset as SteamVR tracks it, when it does)
fn device(height: f32, on_head: bool, head: Option<Target>, raised: f32) -> (Vec3, glam::Quat) {
    let (at, facing) = device_at(height, on_head, head);
    (at + Vec3::Y * raised, facing)
}

fn device_at(height: f32, on_head: bool, head: Option<Target>) -> (Vec3, glam::Quat) {
    if on_head {
        match head {
            Some(h) => (h.position + h.rotation * Vec3::new(0.0, 0.0, 0.08), h.rotation),
            None => (Vec3::new(0.0, height * 0.93, 0.08), glam::Quat::IDENTITY),
        }
    } else {
        (Vec3::new(0.0, height * 0.82 - 0.42, 0.32), glam::Quat::IDENTITY)
    }
}

/// One Leap hand as the avatar's: which, where its wrist is and how it's
/// turned, and its fingers
fn hand(h: &LeapHand, on_head: bool, origin: Vec3, facing: glam::Quat) -> (bool, Target, Hand) {
    let left = h.kind == 0;
    let place = |v: LeapVector| origin + facing * (turned(v, on_head) * 0.001);
    let way = |v: LeapVector| facing * turned(v, on_head);
    let (palm, digits, arm) = (h.palm, h.digits, h.arm);
    let (direction, normal) = (way(palm.direction), way(palm.normal));
    let joints: [[Vec3; 4]; 5] = std::array::from_fn(|f| {
        let b = digits[f].bones;
        [place(b[1].prev_joint), place(b[1].next_joint), place(b[2].next_joint), place(b[3].next_joint)]
    });
    // The wrist: where the forearm ends
    let target = Target { position: place(arm.next_joint), rotation: crate::ik::hand_turn(left, direction, normal) };
    (left, target, super::fingers::from_joints(left, &joints, direction, normal))
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn restart(config: &Config) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !config.leap {
        return;
    }
    let (on_head, height, raised) = (config.leap_on_head, config.user_height.clamp(0.5, 2.5), config.leap_height.clamp(-1.0, 1.0));
    let _ = std::thread::Builder::new().name("theater-puppet-leap".into()).spawn(move || {
        let alive = || GENERATION.load(Ordering::SeqCst) == generation;
        let Some((lib, path)) = Library::first(&candidates()) else {
            super::set_status(Source::Leap, "the Leap software isn't installed (no LeapC.dll)");
            return crate::log("[Puppet] ⚠️ Leap: no LeapC.dll found: install Ultraleap's tracking software");
        };
        let (Some(create), Some(open), Some(poll)) =
            (lib.function::<Create>(c"LeapCreateConnection"), lib.function::<Open>(c"LeapOpenConnection"), lib.function::<Poll>(c"LeapPollConnection"))
        else {
            super::set_status(Source::Leap, "the Leap software's library isn't one Puppet knows");
            return crate::log(&format!("[Puppet] ⚠️ Leap: {} hasn't LeapC's functions", path.display()));
        };
        super::set_status(Source::Leap, "connecting to the Leap service");
        let mut connection: *mut c_void = std::ptr::null_mut();
        if unsafe { create(std::ptr::null(), &mut connection) } != 0 || connection.is_null() || unsafe { open(connection) } != 0 {
            super::set_status(Source::Leap, "can't reach the Leap service");
            return crate::log("[Puppet] ⚠️ Leap: can't open a connection to its service");
        }
        super::set_status(Source::Leap, "waiting for the Leap (is it plugged in?)");
        crate::log(&format!("[Puppet] 🖐️ Leap: connected through {} ({})", path.display(), if on_head { "on the headset" } else { "on the desk" }));
        let policy = lib.function::<Policy>(c"LeapSetPolicyFlags");
        let mut policy_set: Option<Instant> = None;
        let mut received = false;
        while alive() {
            let mut message: LeapMessage = unsafe { std::mem::zeroed() };
            if unsafe { poll(connection, 100, &mut message) } != 0 {
                continue;
            }
            // The device looks up from the desk, or forward from the headset:
            // said again now and then (the service forgets when it restarts)
            if let Some(policy) = policy {
                if policy_set.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
                    let (set, clear) = if on_head { (POLICY_OPTIMIZE_HMD, 0) } else { (0, POLICY_OPTIMIZE_HMD) };
                    unsafe { policy(connection, set, clear) };
                    policy_set = Some(Instant::now());
                }
            }
            let (kind, event) = (message.kind, message.event);
            if kind != EVENT_TRACKING || event.is_null() {
                continue;
            }
            let frame = unsafe { std::ptr::read_unaligned(event as *const LeapTrackingEvent) };
            let (count, first) = (frame.hands as usize, frame.hand);
            let head = if on_head { super::head_from(Source::SteamVr) } else { None };
            let (origin, facing) = device(height, on_head, head, raised);
            let (mut body, mut hands) = (Body::default(), Hands::default());
            for i in 0..count.min(4) {
                if first.is_null() {
                    break;
                }
                let h = unsafe { std::ptr::read_unaligned(first.add(i)) };
                let (left, target, fingers) = hand(&h, on_head, origin, facing);
                if left {
                    body.left_hand = Some(target);
                    hands.left = Some(fingers);
                } else {
                    body.right_hand = Some(target);
                    hands.right = Some(fingers);
                }
            }
            super::set_status(Source::Leap, if count == 0 { "connected, no hand in view".to_string() } else { format!("receiving {count} hand(s)") });
            if count > 0 {
                super::set_body(Source::Leap, body);
                super::set_hands(Source::Leap, hands);
                if !received {
                    received = true;
                    crate::log("[Puppet] 🖐️ Leap: receiving hands");
                }
            }
        }
        if let Some(close) = lib.function::<Close>(c"LeapCloseConnection") {
            unsafe { close(connection) };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> LeapVector {
        LeapVector { x, y, z }
    }

    #[test]
    fn the_structures_are_leapcs_size() {
        assert_eq!(std::mem::size_of::<LeapBone>(), 44);
        assert_eq!(std::mem::size_of::<LeapDigit>(), 184);
        assert_eq!(std::mem::size_of::<LeapPalm>(), 80);
        assert_eq!(std::mem::size_of::<LeapHand>(), 1084);
        assert_eq!(std::mem::size_of::<LeapTrackingEvent>(), 48);
    }

    /// A flat left hand 20 cm over a desk Leap, a little to the left,
    /// fingers pointing at the screen
    fn flat_left() -> LeapHand {
        let mut h: LeapHand = unsafe { std::mem::zeroed() };
        h.kind = 0;
        h.palm.position = v(-100.0, 200.0, 0.0);
        h.palm.normal = v(0.0, -1.0, 0.0);
        h.palm.direction = v(0.0, 0.0, -1.0);
        h.arm.next_joint = v(-100.0, 200.0, 60.0);
        for f in 0..5 {
            // A left hand's thumb is on its right (+X), its little finger on its left
            let x = -100.0 + 40.0 - 20.0 * f as f32;
            for b in 0..4 {
                h.digits[f].bones[b].prev_joint = v(x, 200.0, 40.0 - 30.0 * b as f32);
                h.digits[f].bones[b].next_joint = v(x, 200.0, 10.0 - 30.0 * b as f32);
            }
        }
        h
    }

    #[test]
    fn a_desk_hand_lands_in_front_of_the_avatar() {
        let (origin, facing) = device(1.7, false, None, 0.0);
        let (left, target, fingers) = hand(&flat_left(), false, origin, facing);
        assert!(left);
        // Your left is the avatar's left (+X); in front of it (+Z), below the shoulders
        assert!(target.position.x > 0.05 && target.position.z > 0.2, "{:?}", target.position);
        assert!(target.position.y > 1.0 && target.position.y < 1.39, "{:?}", target.position);
        // Fingers forward, palm down: the left hand turned a quarter about Y
        let tips = target.rotation * Vec3::X;
        let palm = target.rotation * Vec3::NEG_Y;
        assert!(tips.z > 0.99 && palm.y < -0.99, "{tips:?} {palm:?}");
        // Flat: nothing bent
        assert!(fingers.bend[1..].iter().flatten().all(|b| b.abs() < 0.01), "{:?}", fingers.bend);
    }

    #[test]
    fn a_headset_hand_is_ahead_of_the_head() {
        // Seen from a headset: 30 cm ahead (device Y), 10 cm below (device Z)
        let mut h = flat_left();
        h.arm.next_joint = v(100.0, 300.0, 100.0);
        let (origin, facing) = device(1.7, true, None, 0.0);
        let (_, target, _) = hand(&h, true, origin, facing);
        assert!(target.position.x > 0.05 && target.position.z > 0.3 && target.position.y < 1.7 * 0.93 - 0.05, "{:?}", target.position);
    }
}
