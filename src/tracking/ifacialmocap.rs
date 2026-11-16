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
//! format writes `name&value`); the turns are Unity's, in degrees.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{Face, Source, config::Config};

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
    for part in rest.split('|') {
        let Some((key, values)) = part.split_once('#') else { continue };
        let v = numbers(values);
        if v.len() < 3 {
            continue;
        }
        let turn = super::from_unity_euler(v[0], v[1], v[2]);
        match key.trim() {
            "head" => face.head = turn,
            "leftEye" => left = Some(turn),
            "rightEye" => right = Some(turn),
            _ => {}
        }
    }
    if let (Some(l), Some(r)) = (left, right) {
        face.eyes = Some([l, r]);
    }
    Some(face)
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
        while GENERATION.load(Ordering::SeqCst) == generation {
            // The phone sends once asked; asked again now and then, in case the app restarted
            let ip = phone.ip.trim();
            if !ip.is_empty() && asked.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                let _ = socket.send_to(START, (ip, port));
                asked = Some(Instant::now());
            }
            let Ok((len, _)) = socket.recv_from(&mut buf) else { continue };
            let Some(face) = std::str::from_utf8(&buf[..len]).ok().and_then(parse) else { continue };
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
