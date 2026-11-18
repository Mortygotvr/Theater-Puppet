//! VR telemetry: the headset's own telemetry server (the one the owner's
//! Unity avatar app, AvatarStreaming, reads): the tracked devices and both
//! hands' finger skeletons over UDP, with no SteamVR on this PC. For a PC
//! whose graphics card SteamVR's headset streaming won't take (an Intel Arc
//! with a Steam Frame).
//!
//! The server sends to whoever pinged it lately (one byte, 0x01, to each
//! port), big-endian:
//!
//! ```text
//! port 7500, 56 bytes:  u32 device | u32 role | 12 x f32: its 3x4 pose
//!   roles: 1 headset, 2 left hand, 3 right hand, 4 hips, 5 left foot, 6 right foot, 7 chest
//! port 7501, 376 bytes: u32 hand (0 left, 1 right) | 31 x (3 x f32): SteamVR's hand skeleton's joints
//! ```
//!
//! Poses are SteamVR's (metres, Y up, the headset looking along -Z), read as
//! steamvr.rs reads its own.
//!
//! T-pose calibration (as AvatarStreaming's): standing in a T-pose, each
//! device's turn then is the avatar's rest, the headset's place and facing
//! the avatar's, its height the user's. Asked from the Puppet Tracking dock:
//! at once, or when both triggers are pressed (both index fingers curled). A finger's curl is how far its base is from its
//! tip against the finger's length, as AvatarStreaming measures it.

use std::net::UdpSocket;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::{Quat, Vec3};

use super::steamvr::{Room, Strapped, hand_from_curls, position, rotation};
use super::{Body, Hands, Source, Target, config::Config};

/// A T-pose calibration: the room as the headset was then, and each role's
/// turn then (undone, so a device's turn is from how it was in the T-pose)
#[derive(Clone, Copy)]
struct Calibration {
    room: Room,
    undo: [Option<Quat>; 8],
}

/// The calibration (kept while OBS runs, whatever restarts the input)
static CALIBRATION: Mutex<Option<Calibration>> = Mutex::new(None);
/// 0: nothing asked; 1: calibrate at once; 2: when both triggers are pressed
static ASKED: AtomicU8 = AtomicU8::new(0);
/// How far both index fingers must curl to count as the triggers pressed (70 degrees, as AvatarStreaming)
const TRIGGERS: f32 = 70.0 / 90.0;

/// The dock's buttons: calibrate now, or when both triggers are pressed
pub fn calibrate(now: bool) {
    ASKED.store(if now { 1 } else { 2 }, Ordering::SeqCst);
}

/// Back to no calibration (where you stood when it started is the avatar's place)
pub fn reset_calibration() {
    ASKED.store(0, Ordering::SeqCst);
    *CALIBRATION.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// For the dock: "calibrated", "waiting for both triggers", or "not calibrated"
pub fn calibration_state() -> &'static str {
    match ASKED.load(Ordering::SeqCst) {
        1 => "calibrating…",
        2 => "stand in a T-pose and press both triggers",
        _ if CALIBRATION.lock().unwrap_or_else(|e| e.into_inner()).is_some() => "calibrated",
        _ => "not calibrated",
    }
}

/// A device's pose in the avatar's space, before any calibration: the
/// headset's as a head, a controller's as a hand, any other's as itself
fn raw(room: &Room, role: usize, m: &[[f32; 4]; 3]) -> Target {
    match role {
        1 => room.head(m),
        2 => room.hand(m, true),
        3 => room.hand(m, false),
        _ => Target { position: room.place(position(m)), rotation: (room.turn * rotation(m)).normalize() },
    }
}

/// The calibration from the devices' poses now (index: the role), the user
/// in a T-pose; None without the headset
fn calibration_from(poses: &[Option<[[f32; 4]; 3]>; 8]) -> Option<Calibration> {
    let room = Room::from_headset(&poses[1]?);
    let undo = std::array::from_fn(|role| poses[role].filter(|_| role >= 1).map(|m| raw(&room, role, &m).rotation.inverse()));
    Some(Calibration { room, undo })
}

/// A device's pose, calibrated: its turn from how it was in the T-pose
fn calibrated(c: &Calibration, role: usize, m: &[[f32; 4]; 3]) -> Target {
    let t = raw(&c.room, role, m);
    Target { position: t.position, rotation: c.undo[role].map(|u| (t.rotation * u).normalize()).unwrap_or(t.rotation) }
}

pub const POSE_PORT: u16 = 7500;
const SKELETON_PORT_OFFSET: u16 = 1;

/// A pose packet: (role, pose)
fn parse_pose(packet: &[u8]) -> Option<(u32, [[f32; 4]; 3])> {
    if packet.len() != 56 {
        return None;
    }
    let role = u32::from_be_bytes(packet[4..8].try_into().ok()?);
    let mut m = [[0.0f32; 4]; 3];
    for (k, v) in m.iter_mut().flatten().enumerate() {
        *v = f32::from_be_bytes(packet[8 + k * 4..12 + k * 4].try_into().ok()?);
    }
    m.iter().flatten().all(|v| v.is_finite()).then_some((role, m))
}

/// A skeleton packet: (0 left / 1 right, its 31 joints)
fn parse_skeleton(packet: &[u8]) -> Option<(usize, [Vec3; 31])> {
    if packet.len() != 376 {
        return None;
    }
    let hand = u32::from_be_bytes(packet[0..4].try_into().ok()?) as usize;
    if hand > 1 {
        return None;
    }
    let f = |at: usize| f32::from_be_bytes(packet[at..at + 4].try_into().unwrap_or_default());
    let joints: [Vec3; 31] = std::array::from_fn(|i| Vec3::new(f(4 + i * 12), f(8 + i * 12), f(12 + i * 12)));
    joints.iter().all(|j| j.is_finite()).then_some((hand, joints))
}

/// Each finger's curl (thumb first), 0 straight to 1 closed: how near its
/// base comes to its tip, against its length. SteamVR's hand skeleton:
/// thumb joints 2–5, index 7–10, middle 12–15, ring 17–20, little 22–25
/// (each finger's metacarpal before them). AvatarStreaming's thresholds:
/// the straightness of a closed and of a straight finger.
fn curls(joints: &[Vec3; 31]) -> [f32; 5] {
    const FIRST: [usize; 5] = [2, 7, 12, 17, 22];
    const CLOSED: [f32; 5] = [0.84, 0.77, 0.73, 0.77, 0.83];
    const STRAIGHT: [f32; 5] = [0.99, 0.98, 0.99, 0.99, 0.99];
    std::array::from_fn(|f| {
        let j = FIRST[f];
        let length = (joints[j + 1] - joints[j]).length() + (joints[j + 2] - joints[j + 1]).length() + (joints[j + 3] - joints[j + 2]).length();
        if length < 1e-3 {
            return 0.0;
        }
        let straightness = joints[j].distance(joints[j + 3]) / length;
        1.0 - ((straightness - CLOSED[f]) / (STRAIGHT[f] - CLOSED[f])).clamp(0.0, 1.0)
    })
}

/// The body with the hands, hips and feet `k` times as far from the head
/// (the dock's scale: tracked reach to the avatar's)
fn scaled(mut body: Body, k: f32) -> Body {
    let k = k.clamp(0.2, 5.0);
    let Some(head) = body.head.map(|h| h.position) else { return body };
    for t in [&mut body.left_hand, &mut body.right_hand, &mut body.hips, &mut body.left_foot, &mut body.right_foot].into_iter().flatten() {
        t.position = head + (t.position - head) * k;
    }
    body
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn restart(config: &Config) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let server = config.telemetry.clone();
    if !server.enabled {
        return;
    }
    let _ = std::thread::Builder::new().name("theater-puppet-telemetry".into()).spawn(move || {
        let alive = || GENERATION.load(Ordering::SeqCst) == generation;
        let ip = server.ip.trim().to_string();
        if ip.is_empty() {
            return super::set_status(Source::Telemetry, "needs the headset's address");
        }
        let pose_port = if server.port == 0 { POSE_PORT } else { server.port };
        let skeleton_port = pose_port + SKELETON_PORT_OFFSET;
        let (Ok(poses), Ok(skeletons)) = (UdpSocket::bind(("0.0.0.0", 0)), UdpSocket::bind(("0.0.0.0", 0))) else {
            return super::set_status(Source::Telemetry, "can't open a network port");
        };
        let _ = poses.set_nonblocking(true);
        let _ = skeletons.set_nonblocking(true);
        super::set_status(Source::Telemetry, format!("asking {ip}:{pose_port}"));
        crate::log(&format!("[Puppet] 🥽 VR telemetry: asking {ip}:{pose_port} (poses) and :{skeleton_port} (fingers)"));
        let mut pinged: Option<Instant> = None;
        let mut room: Option<Room> = None;
        // Each role's last pose and when (index: the role)
        let mut seen: [Option<([[f32; 4]; 3], Instant)>; 8] = [None; 8];
        // Hips, left foot, right foot: their turn when first seen
        let mut strapped: [Option<Strapped>; 3] = [None; 3];
        let mut fingers: [Option<([f32; 5], Instant)>; 2] = [None, None];
        let mut received = false;
        let mut buf = [0u8; 512];
        while alive() {
            if pinged.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                let _ = poses.send_to(&[1], (ip.as_str(), pose_port));
                let _ = skeletons.send_to(&[1], (ip.as_str(), skeleton_port));
                pinged = Some(Instant::now());
            }
            let mut headset = None;
            let mut got = false;
            while let Ok((len, _)) = poses.recv_from(&mut buf) {
                got = true;
                let Some((role, m)) = parse_pose(&buf[..len]) else { continue };
                if (1..=7).contains(&role) {
                    seen[role as usize] = Some((m, Instant::now()));
                    if role == 1 {
                        headset = Some(m);
                    }
                }
            }
            while let Ok((len, _)) = skeletons.recv_from(&mut buf) {
                got = true;
                if let Some((hand, joints)) = parse_skeleton(&buf[..len]) {
                    fingers[hand] = Some((curls(&joints), Instant::now()));
                }
            }
            // A frame for each new headset pose
            let Some(head) = headset else {
                if !got {
                    std::thread::sleep(Duration::from_millis(2));
                }
                continue;
            };
            let fresh = |role: usize| seen[role].filter(|(_, at)| at.elapsed() < Duration::from_millis(300)).map(|(m, _)| m);
            let curl = |i: usize| fingers[i].filter(|(_, at)| at.elapsed() < Duration::from_millis(300)).map(|(c, _)| c);
            // A T-pose calibration asked for: now, or once both triggers are pressed
            let asked = ASKED.load(Ordering::SeqCst);
            let triggers = curl(0).zip(curl(1)).is_some_and(|(l, r)| l[1] > TRIGGERS && r[1] > TRIGGERS);
            if asked == 1 || (asked == 2 && triggers) {
                let poses: [Option<[[f32; 4]; 3]>; 8] = std::array::from_fn(fresh);
                if let Some(c) = calibration_from(&poses) {
                    *CALIBRATION.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
                    ASKED.store(0, Ordering::SeqCst);
                    // Your height: the headset is at eye height, about 93% of it
                    let eyes = position(&head).y;
                    let mut config = super::config::get();
                    if eyes > 0.8 {
                        config.user_height = (eyes / 0.93).clamp(1.0, 2.2);
                    }
                    let devices = poses.iter().flatten().count();
                    crate::log(&format!("[Puppet] 🥽 VR telemetry: T-pose calibrated ({devices} device(s), height {:.2} m)", config.user_height));
                    super::config::set(config);
                }
            }
            if let Some(c) = *CALIBRATION.lock().unwrap_or_else(|e| e.into_inner()) {
                let at = |role: usize| fresh(role).map(|m| calibrated(&c, role, &m));
                let body = Body { head: at(1), left_hand: at(2), right_hand: at(3), hips: at(4), left_foot: at(5), right_foot: at(6), ..Default::default() };
                let hands = Hands { left: curl(0).map(|c| hand_from_curls(c, None)), right: curl(1).map(|c| hand_from_curls(c, None)) };
                let controllers = body.left_hand.is_some() as u32 + body.right_hand.is_some() as u32;
                let trackers = [&body.hips, &body.left_foot, &body.right_foot].iter().filter(|t| t.is_some()).count();
                let with_fingers = hands.left.is_some() as u32 + hands.right.is_some() as u32;
                super::set_body(Source::Telemetry, scaled(body, super::config::get().telemetry_scale));
                if with_fingers > 0 {
                    super::set_hands(Source::Telemetry, hands);
                }
                super::set_status(Source::Telemetry, format!("calibrated: the headset, {controllers} controller(s) ({with_fingers} with fingers), {trackers} tracker(s)"));
                continue;
            }
            // Not calibrated: where you stand and face when it starts is the avatar's place and forward
            let room = *room.get_or_insert_with(|| Room::from_headset(&head));
            for (slot, role) in [4usize, 5, 6].into_iter().enumerate() {
                match fresh(role) {
                    Some(m) if strapped[slot].is_none() => strapped[slot] = Some(Strapped { device: role as u32, at_start: rotation(&m) }),
                    None => strapped[slot] = None,
                    _ => {}
                }
            }
            let tracker = |slot: usize, role: usize| strapped[slot].and_then(|s| fresh(role).map(|m| s.target(&room, &m)));
            let body = Body {
                head: Some(room.head(&head)),
                left_hand: fresh(2).map(|m| room.hand(&m, true)),
                right_hand: fresh(3).map(|m| room.hand(&m, false)),
                hips: tracker(0, 4),
                left_foot: tracker(1, 5),
                right_foot: tracker(2, 6),
                ..Default::default()
            };
            let curled = |i: usize| fingers[i].filter(|(_, at)| at.elapsed() < Duration::from_millis(300)).map(|(c, _)| hand_from_curls(c, None));
            let hands = Hands { left: curled(0), right: curled(1) };
            let controllers = body.left_hand.is_some() as u32 + body.right_hand.is_some() as u32;
            let with_fingers = hands.left.is_some() as u32 + hands.right.is_some() as u32;
            let trackers = strapped.iter().flatten().count();
            super::set_body(Source::Telemetry, scaled(body, super::config::get().telemetry_scale));
            if with_fingers > 0 {
                super::set_hands(Source::Telemetry, hands);
            }
            super::set_status(Source::Telemetry, format!("receiving the headset, {controllers} controller(s) ({with_fingers} with fingers), {trackers} tracker(s)"));
            if !received {
                received = true;
                crate::log("[Puppet] 🥽 VR telemetry: receiving");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device at `p`, turned `yaw` degrees about Y and `roll` about Z
    fn device(p: Vec3, yaw: f32, roll: f32) -> [[f32; 4]; 3] {
        let r = glam::Mat3::from_quat(Quat::from_rotation_y(yaw.to_radians()) * Quat::from_rotation_z(roll.to_radians()));
        let c = r.to_cols_array_2d();
        [[c[0][0], c[1][0], c[2][0], p.x], [c[0][1], c[1][1], c[2][1], p.y], [c[0][2], c[1][2], c[2][2], p.z]]
    }

    #[test]
    fn a_t_pose_calibration_makes_the_t_pose_the_rest() {
        // Facing 30 degrees off, controllers held any way round, a hips tracker strapped on sideways
        let mut poses: [Option<[[f32; 4]; 3]>; 8] = [None; 8];
        poses[1] = Some(device(Vec3::new(1.0, 1.6, 2.0), 30.0, 0.0));
        poses[2] = Some(device(Vec3::new(0.3, 1.4, 2.5), 75.0, 40.0));
        poses[3] = Some(device(Vec3::new(1.6, 1.4, 1.5), -60.0, -25.0));
        poses[4] = Some(device(Vec3::new(1.0, 1.0, 2.0), 120.0, 90.0));
        let c = calibration_from(&poses).unwrap();
        // In the T-pose: every turn is the rest, the head at the avatar's place
        for role in 1..=4 {
            let t = calibrated(&c, role, &poses[role].unwrap());
            assert!(t.rotation.angle_between(Quat::IDENTITY) < 1e-3, "role {role}");
        }
        let head = calibrated(&c, 1, &poses[1].unwrap());
        assert!((head.position - Vec3::new(0.0, 1.6, 0.0)).length() < 1e-4);
        // Then the left controller turned 20 degrees more about the room's up: the hand turns 20 degrees about the avatar's
        let moved = calibrated(&c, 2, &device(Vec3::new(0.3, 1.4, 2.5), 95.0, 40.0));
        let (axis, angle) = moved.rotation.to_axis_angle();
        assert!((angle.to_degrees() - 20.0).abs() < 0.1 && axis.y.abs() > 0.999, "{axis:?} {}", angle.to_degrees());
        // Nothing without the headset
        assert!(calibration_from(&[None; 8]).is_none());
    }

    #[test]
    fn the_scale_spreads_from_the_head() {
        let at = |p: Vec3| Some(Target { position: p, rotation: Quat::IDENTITY });
        let body = Body { head: at(Vec3::new(0.0, 1.0, 0.0)), left_hand: at(Vec3::new(0.2, 0.8, 0.3)), ..Default::default() };
        let twice = scaled(body, 2.0);
        assert_eq!(twice.head.unwrap().position, Vec3::new(0.0, 1.0, 0.0));
        assert!((twice.left_hand.unwrap().position - Vec3::new(0.4, 0.6, 0.6)).length() < 1e-6);
        assert_eq!(scaled(body, 1.0), body);
    }

    #[test]
    fn a_pose_packet_is_read() {
        // As the server sent it: device 3, the left controller
        let pose = [0.988f32, 0.145, -0.057, -0.149, -0.085, 0.194, -0.977, 0.928, -0.131, 0.97, 0.204, -0.322];
        let mut packet = vec![0, 0, 0, 3, 0, 0, 0, 2];
        for v in pose {
            packet.extend_from_slice(&v.to_be_bytes());
        }
        let (role, m) = parse_pose(&packet).unwrap();
        assert_eq!(role, 2);
        assert_eq!((m[0][3], m[1][3], m[2][2]), (-0.149, 0.928, 0.204));
        assert!(parse_pose(&packet[..40]).is_none());
    }

    /// A hand skeleton: every finger straight along -Z, or bent at each joint
    fn skeleton(bent: [f32; 5]) -> [Vec3; 31] {
        let mut j = [Vec3::ZERO; 31];
        for (f, first) in [2usize, 7, 12, 17, 22].into_iter().enumerate() {
            let mut at = Vec3::new(0.02 * f as f32, 0.0, 0.0);
            let mut dir = Vec3::NEG_Z;
            j[first] = at;
            for k in 1..4 {
                at += dir * 0.03;
                j[first + k] = at;
                dir = glam::Quat::from_rotation_x(-bent[f]) * dir;
            }
        }
        j
    }

    #[test]
    fn a_skeleton_packet_gives_curls() {
        let mut packet = 1u32.to_be_bytes().to_vec();
        for joint in skeleton([0.0, 1.2, 0.0, 0.0, 0.0]) {
            for v in joint.to_array() {
                packet.extend_from_slice(&v.to_be_bytes());
            }
        }
        let (hand, joints) = parse_skeleton(&packet).unwrap();
        assert_eq!(hand, 1);
        let c = curls(&joints);
        // The index closed, the rest straight
        assert!(c[1] > 0.95, "{c:?}");
        assert!(c[0] < 0.01 && c[2] < 0.01 && c[4] < 0.01, "{c:?}");
        // Partly bent: partly curled
        let half = curls(&skeleton([0.0, 0.45, 0.0, 0.0, 0.0]));
        assert!(half[1] > 0.1 && half[1] < 0.9, "{half:?}");
        assert!(parse_skeleton(&packet[..100]).is_none());
    }
}
