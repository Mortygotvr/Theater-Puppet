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
//! steamvr.rs reads its own. A finger's curl is how far its base is from its
//! tip against the finger's length, as AvatarStreaming measures it.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::Vec3;

use super::steamvr::{Room, Strapped, hand_from_curls, rotation};
use super::{Body, Hands, Source, config::Config};

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
            // Where you stand and face when it starts is the avatar's place and forward
            let room = *room.get_or_insert_with(|| Room::from_headset(&head));
            let fresh = |role: usize| seen[role].filter(|(_, at)| at.elapsed() < Duration::from_millis(300)).map(|(m, _)| m);
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
            super::set_body(Source::Telemetry, body);
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
