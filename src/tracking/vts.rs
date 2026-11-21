//! MeowFace (Android), and anything else speaking VTube Studio's phone
//! protocol: the PC asks the phone (UDP, its port 21412) to send for a few
//! seconds, again and again, and the phone answers with JSON to the port the
//! request names:
//!
//! ```text
//! -> {"messageType":"iOSTrackingDataRequest","time":10,"sentBy":"Theater Puppet","ports":[50506]}
//! <- {"Timestamp":…,"FaceFound":true,"Rotation":{"x":…,"y":…,"z":…},"Position":{…},
//!     "EyeLeft":{"x":…,"y":…,"z":…},"EyeRight":{…},"BlendShapes":[{"k":"EyeBlinkLeft","v":0.1},…]}
//! ```
//!
//! Shapes are ARKit's, capitalised, 0–1; the turns are Unity's, in degrees.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{Face, Source, config::Config};

/// Where the phone sends to on this PC
const LISTEN_PORT: u16 = 50506;
/// The phone keeps sending this long after a request (seconds)
const ASK_FOR: u64 = 10;

fn turn(v: &serde_json::Value) -> Option<glam::Quat> {
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    Some(super::from_unity_euler(f("x")?, f("y")?, f("z")?))
}

/// A packet's face; None if it isn't one, or the phone sees no face
pub fn parse(text: &str) -> Option<Face> {
    match read(text) {
        Packet::Face(face, _) => Some(face),
        _ => None,
    }
}

/// What a packet is
pub enum Packet {
    /// A face, and the shape names in it that aren't ARKit's (skipped)
    Face(Face, Vec<String>),
    /// Shapes, none of them ARKit's: their names
    Unknown(Vec<String>),
    /// No face in view, or not a packet of this protocol
    Nothing,
}

pub fn read(text: &str) -> Packet {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Packet::Nothing };
    if v.get("FaceFound").and_then(|f| f.as_bool()) == Some(false) {
        return Packet::Nothing;
    }
    let Some(shapes) = v.get("BlendShapes").and_then(|b| b.as_array()) else { return Packet::Nothing };
    let mut face = Face::default();
    let mut found = 0;
    let mut unknown = Vec::new();
    for shape in shapes {
        let (Some(k), Some(val)) = (shape.get("k").and_then(|k| k.as_str()), shape.get("v").and_then(|x| x.as_f64())) else { continue };
        match super::arkit_index(k) {
            Some(i) => {
                face.shapes[i] = (val as f32).clamp(0.0, 1.0);
                found += 1;
            }
            None => unknown.push(k.to_string()),
        }
    }
    if found == 0 {
        return if unknown.is_empty() { Packet::Nothing } else { Packet::Unknown(unknown) };
    }
    if let Some(head) = v.get("Rotation").and_then(turn) {
        face.head = head;
    }
    if let (Some(l), Some(r)) = (v.get("EyeLeft").and_then(turn), v.get("EyeRight").and_then(turn)) {
        face.eyes = Some([l, r]);
    }
    Packet::Face(face, unknown)
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn restart(config: &Config) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let phone = config.meowface.clone();
    if !phone.enabled {
        return;
    }
    let _ = std::thread::Builder::new().name("theater-puppet-meowface".into()).spawn(move || {
        // Another program on the port: tried again every second
        let mut told = false;
        let socket = loop {
            match UdpSocket::bind(("0.0.0.0", LISTEN_PORT)) {
                Ok(s) => break s,
                Err(e) => {
                    super::set_status(Source::MeowFace, format!("port {LISTEN_PORT} is in use by another program: close it"));
                    if !told {
                        told = true;
                        crate::log(&format!("[Puppet] ⚠️ MeowFace: can't listen on UDP {LISTEN_PORT} ({e}): another program has it; trying again every second"));
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    if GENERATION.load(Ordering::SeqCst) != generation {
                        return;
                    }
                }
            }
        };
        let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
        let port = if phone.port == 0 { 21412 } else { phone.port };
        let ip = phone.ip.trim().to_string();
        if ip.is_empty() {
            super::set_status(Source::MeowFace, "needs the phone's address");
        } else {
            super::set_status(Source::MeowFace, format!("asking {ip}:{port}"));
        }
        crate::log(&format!("[Puppet] 📱 MeowFace: asking {ip}:{port}, listening on UDP {LISTEN_PORT}"));
        let request = serde_json::json!({ "messageType": "iOSTrackingDataRequest", "time": ASK_FOR, "sentBy": "Theater Puppet", "ports": [LISTEN_PORT] }).to_string();
        let mut asked: Option<Instant> = None;
        let mut received = false;
        let mut buf = [0u8; 16384];
        while GENERATION.load(Ordering::SeqCst) == generation {
            // Asked again well before the phone stops
            if !ip.is_empty() && asked.is_none_or(|t| t.elapsed() > Duration::from_secs(ASK_FOR / 3)) {
                let _ = socket.send_to(request.as_bytes(), (ip.as_str(), port));
                asked = Some(Instant::now());
            }
            let Ok((len, _)) = socket.recv_from(&mut buf) else { continue };
            let Ok(text) = std::str::from_utf8(&buf[..len]) else { continue };
            match read(text) {
                Packet::Face(face, unknown) => {
                    let known = face.shapes.iter().filter(|w| **w > 0.0).count();
                    super::set_face(Source::MeowFace, face);
                    super::set_status(Source::MeowFace, "receiving");
                    if !received {
                        received = true;
                        // Which shapes it sends that Puppet doesn't know (they're skipped)
                        let skipped = if unknown.is_empty() { String::new() } else { format!("; not ARKit's, skipped: {}", unknown.join(", ")) };
                        crate::log(&format!("[Puppet] 📱 MeowFace: receiving the face ({known} ARKit shape(s) moving{skipped})"));
                    }
                }
                Packet::Unknown(names) => {
                    super::set_status(Source::MeowFace, format!("connected, but its shapes aren't ARKit's ({}…): Puppet can't use them", names.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
                    if !received {
                        received = true;
                        crate::log(&format!("[Puppet] ⚠️ MeowFace: its shapes aren't ARKit's, so none is used: {}", names.join(", ")));
                    }
                }
                Packet::Nothing => super::set_status(Source::MeowFace, "connected, no face in view"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::arkit_index;

    #[test]
    fn a_packet_is_read() {
        let json = r#"{"Timestamp":1687338063000,"Hotkey":-1,"FaceFound":true,
            "Rotation":{"x":10.0,"y":-20.0,"z":3.0},"Position":{"x":0.1,"y":0.2,"z":-0.5},
            "EyeLeft":{"x":1.0,"y":2.0,"z":0.0},"EyeRight":{"x":1.0,"y":2.5,"z":0.0},
            "BlendShapes":[{"k":"EyeBlinkLeft","v":0.8},{"k":"JawOpen","v":0.33},{"k":"MouthSmileRight","v":1.4},{"k":"Unknown","v":0.5}]}"#;
        let face = parse(json).unwrap();
        assert_eq!(face.shapes[arkit_index("eyeBlinkLeft").unwrap()], 0.8);
        assert_eq!(face.shapes[arkit_index("jawOpen").unwrap()], 0.33);
        assert_eq!(face.shapes[arkit_index("mouthSmileRight").unwrap()], 1.0);
        // Unity +X: nodding down
        assert!((face.head * glam::Vec3::Z).y < -0.1);
        assert!(face.eyes.is_some());
    }

    #[test]
    fn no_face_is_no_packet() {
        assert!(parse(r#"{"FaceFound":false,"BlendShapes":[{"k":"JawOpen","v":0.3}]}"#).is_none());
        assert!(parse("not json").is_none());
        assert!(parse(r#"{"BlendShapes":[]}"#).is_none());
        // Shapes none of which are ARKit's: said, not taken for "no face"
        match read(r#"{"FaceFound":true,"BlendShapes":[{"k":"MouthOpenWide","v":0.3}]}"#) {
            Packet::Unknown(names) => assert_eq!(names, vec!["MouthOpenWide".to_string()]),
            _ => panic!("not told apart"),
        }
        // Some ARKit's, some not: the face, and the ones skipped
        match read(r#"{"FaceFound":true,"BlendShapes":[{"k":"JawOpen","v":0.3},{"k":"Wink","v":1}]}"#) {
            Packet::Face(face, unknown) => {
                assert_eq!(face.shapes[arkit_index("jawOpen").unwrap()], 0.3);
                assert_eq!(unknown, vec!["Wink".to_string()]);
            }
            _ => panic!("no face"),
        }
    }
}
