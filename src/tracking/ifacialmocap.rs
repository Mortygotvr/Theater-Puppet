//! iFacialMocap (iPhone): ARKit's shapes and the head's turn over UDP.
//!
//! The phone sends text to this PC's port 49983, once it's been asked by a
//! start message to its own port 49983 (so the phone's address is needed,
//! unless the app is given this PC's):
//!
//! ```text
//! mouthSmile_R-0|eyeLookOut_L-0|…|jawOpen-12|=head#rx,ry,rz,tx,ty,tz|rightEye#rx,ry,rz|leftEye#rx,ry,rz|
//! ```
//!
//! Shapes are ARKit's with `_L` / `_R` for Left / Right, 0–100 (a newer
//! format writes `name&value`); the turns are Unity's, in degrees; the head's
//! tx, ty, tz its place from the phone, metres (Unity's axes, mirrored like
//! the turns). That place is the head's middle, above the neck: a nod or a
//! tilt swings it, so the neck's place is worked out from it and the head's
//! turn (else every nod would lean the whole spine), steadied, and moved
//! from its first place (the head at rest; the dock's "Re-centre head" takes
//! the next one).

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::Vec3;

use super::mediapipe::Steady;
use super::{Body, Face, Source, Target, config::Config};

/// From the neck's pivot to the place iFacialMocap gives (the head's middle),
/// the head looking ahead: up, a little forward (metres)
const NECK_TO_HEAD: Vec3 = Vec3::new(0.0, 0.08, 0.01);

/// Where the neck is, from where the head's middle is and how it's turned
pub fn neck(head_middle: Vec3, turn: glam::Quat) -> Vec3 {
    head_middle - turn * NECK_TO_HEAD
}

const START: &[u8] = b"iFacialMocap_sahuasouryya9218sauhuiayeta91555dy3719";

/// `mouthSmile_R` -> the place of mouthSmileRight in ARKIT
fn shape_index(name: &str) -> Option<usize> {
    let arkit = match name.rsplit_once('_') {
        Some((base, "L")) => format!("{base}Left"),
        Some((base, "R")) => format!("{base}Right"),
        _ => name.to_string(),
    };
    super::arkit_index(&arkit)
}

fn numbers(text: &str) -> Vec<f32> {
    text.split(',').filter_map(|v| v.trim().parse().ok()).collect()
}

/// A packet's face, or None if it isn't one
pub fn parse(text: &str) -> Option<Face> {
    parse_with_place(text).map(|(face, _)| face)
}

/// A packet's face, and where the head is (the avatar's axes: X its left,
/// Y up, Z toward the phone; metres) when the packet says
pub fn parse_with_place(text: &str) -> Option<(Face, Option<Vec3>)> {
    let (shapes, rest) = text.split_once('=').unwrap_or((text, ""));
    let mut face = Face::default();
    let mut found = 0;
    for item in shapes.split('|') {
        // name&value, or name-value (the name has no '-')
        let Some((name, value)) = item.split_once('&').or_else(|| item.rsplit_once('-')) else { continue };
        let (Some(i), Ok(v)) = (shape_index(name.trim()), value.trim().parse::<f32>()) else { continue };
        face.shapes[i] = (v / 100.0).clamp(0.0, 1.0);
        found += 1;
    }
    if found == 0 {
        return None;
    }
    let (mut left, mut right) = (None, None);
    let mut place = None;
    for part in rest.split('|') {
        let Some((key, values)) = part.split_once('#') else { continue };
        let v = numbers(values);
        if v.len() < 3 {
            continue;
        }
        let turn = super::from_unity_euler(v[0], v[1], v[2]);
        match key.trim() {
            "head" => {
                face.head = turn;
                if v.len() >= 6 {
                    // Mirrored across X, like the turn
                    place = Some(Vec3::new(-v[3], v[4], v[5]));
                }
            }
            "leftEye" => left = Some(turn),
            "rightEye" => right = Some(turn),
            _ => {}
        }
    }
    if let (Some(l), Some(r)) = (left, right) {
        face.eyes = Some([l, r]);
    }
    Some((face, place.filter(|p: &Vec3| p.is_finite())))
}

/// The next place seen is the head at rest again (the dock's "Re-centre head")
static RECENTRE: AtomicBool = AtomicBool::new(false);

pub fn recentre() {
    RECENTRE.store(true, Ordering::SeqCst);
}

/// Bumped at each restart: a listener stops when it's no longer its own
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn restart(config: &Config) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let phone = config.ifacialmocap.clone();
    if !phone.enabled {
        return;
    }
    let _ = std::thread::Builder::new().name("theater-puppet-ifacialmocap".into()).spawn(move || {
        let port = if phone.port == 0 { 49983 } else { phone.port };
        // Another program on the port (only one can listen): tried again
        // every second, so it starts as soon as that one closes
        let mut told = false;
        let socket = loop {
            match UdpSocket::bind(("0.0.0.0", port)) {
                Ok(s) => break s,
                Err(e) => {
                    super::set_status(Source::IFacialMocap, format!("port {port} is in use by another program: close it"));
                    if !told {
                        told = true;
                        crate::log(&format!("[Puppet] ⚠️ iFacialMocap: can't listen on UDP {port} ({e}): another program has it; trying again every second"));
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    if GENERATION.load(Ordering::SeqCst) != generation {
                        return;
                    }
                }
            }
        };
        let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
        super::set_status(Source::IFacialMocap, format!("listening on port {port}"));
        crate::log(&format!("[Puppet] 📱 iFacialMocap: listening on UDP {port}"));
        let mut asked: Option<Instant> = None;
        let mut received = false;
        let mut buf = [0u8; 8192];
        // The neck at rest; the neck's place steadied, and when it last came
        let mut neutral: Option<Vec3> = None;
        let mut steady = [Steady::default(); 3];
        let mut last: Option<Instant> = None;
        while GENERATION.load(Ordering::SeqCst) == generation {
            // The phone sends once asked; asked again now and then, in case the app restarted
            let ip = phone.ip.trim();
            if !ip.is_empty() && asked.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                let _ = socket.send_to(START, (ip, port));
                asked = Some(Instant::now());
            }
            let Ok((len, _)) = socket.recv_from(&mut buf) else { continue };
            let Some((face, place)) = std::str::from_utf8(&buf[..len]).ok().and_then(parse_with_place) else { continue };
            // Where the head is: standing at the user's eye height, moved as it moved from rest
            if let Some(at) = place {
                // The neck, not the head's middle (a nod isn't a lean), steadied:
                // still at rest, quick on a real move
                let dt = last.map(|t| t.elapsed().as_secs_f32()).unwrap_or(1.0 / 60.0).clamp(0.001, 0.1);
                last = Some(Instant::now());
                let raw = neck(at, face.head);
                let at = Vec3::from(std::array::from_fn(|i| steady[i].step(raw[i], dt, 1.0, 10.0)));
                if RECENTRE.swap(false, Ordering::SeqCst) {
                    neutral = None;
                }
                let rest = *neutral.get_or_insert(at);
                let height = super::config::get().user_height.clamp(0.5, 2.5);
                let head = Target { position: Vec3::new(0.0, height * 0.93, 0.0) + (at - rest), rotation: face.head };
                super::set_body(Source::IFacialMocap, Body { head: Some(head), ..Default::default() });
            }
            super::set_face(Source::IFacialMocap, face);
            super::set_status(Source::IFacialMocap, "receiving");
            if !received {
                received = true;
                crate::log("[Puppet] 📱 iFacialMocap: receiving the face");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::arkit_index;

    const SAMPLE: &str = "mouthSmile_R-0|eyeLookOut_L-0|mouthUpperUp_L-11|eyeWide_R-0|mouthClose-8|mouthPucker-4|mouthRollLower-9|eyeBlink_R-7|eyeLookDown_L-17|cheekSquint_R-11|eyeBlink_L-70|tongueOut-0|jawRight-0|eyeLookIn_R-6|cheekSquint_L-11|mouthDimple_L-10|mouthPress_L-4|eyeSquint_L-11|mouthRight-0|mouthShrugLower-9|eyeLookUp_R-0|eyeLookOut_R-0|mouthPress_R-5|cheekPuff-2|jawForward-11|mouthLowerDown_L-9|mouthFrown_L-6|mouthShrugUpper-26|browOuterUp_L-4|browInnerUp-20|mouthDimple_R-10|browDown_R-0|mouthUpperUp_R-10|mouthRollUpper-8|mouthFunnel-12|mouthStretch_R-21|mouthFrown_R-13|eyeLookDown_R-17|jawOpen-12|jawLeft-0|browDown_L-0|mouthSmile_L-0|noseSneer_R-18|mouthLowerDown_R-8|noseSneer_L-21|eyeWide_L-0|mouthStretch_L-21|browOuterUp_R-4|eyeLookIn_L-4|eyeSquint_R-11|eyeLookUp_L-0|mouthLeft-1|=head#-21.488958,-6.038993,-6.6019735,-0.030653415,-0.10287084,-0.6584072|rightEye#6.0297494,2.4403017,0.25649446|leftEye#6.034996,-1.6660284,-0.17520553|";

    #[test]
    fn a_packet_is_read() {
        let face = parse(SAMPLE).unwrap();
        let at = |name: &str| face.shapes[arkit_index(name).unwrap()];
        assert_eq!(at("eyeBlinkLeft"), 0.70);
        assert_eq!(at("eyeBlinkRight"), 0.07);
        assert_eq!(at("jawOpen"), 0.12);
        assert_eq!(at("browInnerUp"), 0.20);
        assert_eq!(at("mouthStretchRight"), 0.21);
        // Head: nodding up (Unity -X): the forward axis rises
        let forward = face.head * glam::Vec3::Z;
        assert!(forward.y > 0.3, "{forward:?}");
        assert!(face.eyes.is_some());
        // The head's place: tx mirrored, metres
        let (_, place) = parse_with_place(SAMPLE).unwrap();
        let p = place.unwrap();
        assert!((p - Vec3::new(0.030653415, -0.10287084, -0.6584072)).length() < 1e-6, "{p:?}");
        assert!(parse_with_place("jawOpen-12|=head#0,0,0|").unwrap().1.is_none());
    }

    #[test]
    fn a_nod_moves_the_head_not_the_neck() {
        let rest = Vec3::new(0.0, 0.0, -1.0);
        let still = neck(rest, glam::Quat::IDENTITY);
        // Nodding 20° down about the neck: the head's middle swings forward and down
        let nod = glam::Quat::from_rotation_x(0.35);
        let middle = still + nod * NECK_TO_HEAD;
        assert!((middle - rest).length() > 0.02, "the middle moved {:?}", middle - rest);
        assert!((neck(middle, nod) - still).length() < 1e-6, "the neck stays");
    }

    #[test]
    fn the_newer_format_and_junk() {
        let face = parse("trackingStatus-1|eyeBlink_L&50|jawOpen&25|=head#0,10,0,0,0,0|").unwrap();
        assert_eq!(face.shapes[arkit_index("eyeBlinkLeft").unwrap()], 0.5);
        assert_eq!(face.shapes[arkit_index("jawOpen").unwrap()], 0.25);
        assert!(parse("hello").is_none());
        assert!(parse("").is_none());
    }
}
