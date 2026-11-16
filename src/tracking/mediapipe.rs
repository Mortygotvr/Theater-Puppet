//! MediaPipe: a camera's face (ARKit's shapes, the head), body (arms, feet)
//! and hands (fingers).
//!
//! MediaPipe itself runs in a helper beside OBS (data/mediapipe/
//! puppet_mediapipe.py, in a Python that has mediapipe and opencv), which
//! opens the camera and writes what it sees, one JSON line per frame; this
//! starts it, stops it, and turns its lines into the avatar's space.
//!
//! The camera looks at you: what's to the right in its picture is your left,
//! which is the avatar's left (+X); toward the camera is the avatar's
//! forward (+Z).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glam::{Mat3, Quat, Vec3};
use serde_json::Value;

use super::{Body, Face, Hand, Hands, Pick, Source, Target, config::Config};

/// What a line of the helper's says
#[derive(Default)]
pub struct Seen {
    pub face: Option<Face>,
    pub body: Option<Body>,
    pub hands: Option<Hands>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub fps: Option<f32>,
}

/// From the camera's lines to the avatar's space, for a user `height` tall
pub struct Mapper {
    height: f32,
    /// Where the face was when it was first seen (cm, the camera's space):
    /// the avatar's head at rest
    neutral: Option<Vec3>,
    /// How far the head is from there now (metres)
    shift: Vec3,
}

fn point(v: &Value) -> Option<Vec3> {
    let a = v.as_array()?;
    Some(Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32))
}

fn pair(v: &Value) -> Option<(f32, f32)> {
    let a = v.as_array()?;
    Some((a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32))
}

/// A world landmark (X right in the picture, Y down, Z away) in the avatar's space
fn landmark(v: &Value) -> Option<Vec3> {
    point(v).map(|p| Vec3::new(p.x, -p.y, -p.z))
}

impl Mapper {
    pub fn new(height: f32) -> Mapper {
        Mapper { height: height.clamp(0.5, 2.5), neutral: None, shift: Vec3::ZERO }
    }

    pub fn line(&mut self, text: &str) -> Seen {
        let mut seen = Seen::default();
        let Ok(v) = serde_json::from_str::<Value>(text) else { return seen };
        seen.status = v.get("status").and_then(|s| s.as_str()).map(String::from);
        seen.error = v.get("error").and_then(|s| s.as_str()).map(String::from);
        seen.fps = v.get("fps").and_then(|f| f.as_f64()).map(|f| f as f32);
        let mut body = Body::default();
        if let Some(face) = v.get("face") {
            if let Some((face, head)) = self.face(face) {
                body.head = Some(head);
                seen.face = Some(face);
            }
        }
        // Each hand the hand model sees: (is the left, its turn, its fingers)
        let mut hands: Vec<(Option<(f32, f32)>, String, Vec<Vec3>)> = Vec::new();
        for h in v.get("hands").and_then(|h| h.as_array()).into_iter().flatten() {
            let points: Vec<Vec3> = h.get("world").and_then(|w| w.as_array()).into_iter().flatten().filter_map(landmark).collect();
            if points.len() == 21 {
                hands.push((h.get("wrist").and_then(pair), h.get("label").and_then(|l| l.as_str()).unwrap_or("").to_string(), points));
            }
        }
        let mut wrists: [Option<(f32, f32)>; 2] = [None, None];
        if let Some(pose) = v.get("pose") {
            let world: Vec<(Vec3, f32)> = pose
                .get("world")
                .and_then(|w| w.as_array())
                .into_iter()
                .flatten()
                .filter_map(|l| Some((landmark(l)?, l.get(3)?.as_f64()? as f32)))
                .collect();
            if world.len() == 33 {
                self.pose(&world, &mut body);
                let in_picture: Vec<(f32, f32)> = pose.get("wrists").and_then(|w| w.as_array()).into_iter().flatten().filter_map(pair).collect();
                if in_picture.len() == 2 {
                    wrists = [body.left_hand.map(|_| in_picture[0]), body.right_hand.map(|_| in_picture[1])];
                }
            }
        }
        let mut fingers = Hands::default();
        for (wrist, label, points) in &hands {
            // Which hand: the body's wrist it's on, when the body is seen;
            // else the hand model's word (it takes the picture for a mirror's:
            // its "Right" is your left)
            let near = |w: Option<(f32, f32)>| match (w, wrist) {
                (Some(a), Some(b)) => Some((a.0 - b.0).hypot(a.1 - b.1)),
                _ => None,
            };
            let left = match (near(wrists[0]), near(wrists[1])) {
                (Some(l), Some(r)) => l < r,
                (Some(l), None) if l < 0.15 => true,
                (None, Some(r)) if r < 0.15 => false,
                _ => label == "Right",
            };
            let (turn, hand) = one_hand(left, points);
            if left {
                fingers.left = Some(hand);
                if let Some(t) = &mut body.left_hand {
                    t.rotation = turn;
                }
            } else {
                fingers.right = Some(hand);
                if let Some(t) = &mut body.right_hand {
                    t.rotation = turn;
                }
            }
        }
        if fingers != Hands::default() {
            seen.hands = Some(fingers);
        }
        if body != Body::default() {
            seen.body = Some(body);
        }
        seen
    }

    /// The face's shapes, and the head (where, and how it's turned)
    fn face(&mut self, v: &Value) -> Option<(Face, Target)> {
        let mut face = Face::default();
        let mut found = 0;
        for (name, value) in v.get("shapes")?.as_object()? {
            let (Some(i), Some(value)) = (super::arkit_index(name), value.as_f64()) else { continue };
            let value = value as f32;
            // MediaPipe's closed eye stops short of 1
            face.shapes[i] = if name.starts_with("eyeBlink") { (value - 0.08) / 0.5 } else { value }.clamp(0.0, 1.0);
            found += 1;
        }
        let m: Vec<f32> = v.get("matrix")?.as_array()?.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
        if found == 0 || m.len() != 16 {
            return None;
        }
        // The camera's space is the avatar's (X: your left, Y up, Z toward the camera)
        let column = |j: usize| Vec3::new(m[j], m[4 + j], m[8 + j]).normalize_or_zero();
        face.head = Quat::from_mat3(&Mat3::from_cols(column(0), column(1), column(2))).normalize();
        if !face.head.is_finite() {
            face.head = Quat::IDENTITY;
        }
        let at = Vec3::new(m[3], m[7], m[11]);
        let neutral = *self.neutral.get_or_insert(at);
        // (how near the camera is guessed from the face's size: half trusted)
        self.shift = (at - neutral) * 0.01 * Vec3::new(1.0, 1.0, 0.5);
        let head = Target { position: Vec3::new(0.0, self.height * 0.93, 0.0) + self.shift, rotation: face.head };
        Some((face, head))
    }

    /// The arms and the feet, from the body's 33 landmarks (place, how visible)
    fn pose(&self, world: &[(Vec3, f32)], body: &mut Body) {
        let seen = |i: usize, least: f32| (world[i].1 >= least).then_some(world[i].0);
        // The shoulders' middle is where the avatar's is, moved with the head
        let shoulders = (world[11].0 + world[12].0) * 0.5;
        let chest = Vec3::new(0.0, self.height * 0.82, 0.0) + self.shift;
        let arm = |wrist: usize, elbow: usize, index: usize, little: usize, left: bool| -> (Option<Target>, Option<Vec3>) {
            let Some(w) = seen(wrist, 0.5) else { return (None, None) };
            let (i, l) = (world[index].0 - w, world[little].0 - w);
            let across = i.cross(l);
            let turn = crate::ik::hand_turn(left, (i + l) * 0.5, if left { -across } else { across });
            (Some(Target { position: chest + (w - shoulders), rotation: turn }), seen(elbow, 0.3).map(|e| chest + (e - shoulders)))
        };
        (body.left_hand, body.left_elbow) = arm(15, 13, 19, 17, true);
        (body.right_hand, body.right_elbow) = arm(16, 14, 20, 18, false);
        // The feet, when the camera sees that far (the landmarks' origin is the hips' middle)
        let hips = Vec3::new(0.0, self.height * 0.53, 0.0) + self.shift;
        let foot = |ankle: usize| seen(ankle, 0.6).map(|a| Target { position: hips + a, rotation: Quat::IDENTITY });
        body.left_foot = foot(27);
        body.right_foot = foot(28);
    }
}

/// One hand from its 21 landmarks (the avatar's directions): how it's
/// turned, and its fingers
fn one_hand(left: bool, p: &[Vec3]) -> (Quat, Hand) {
    // Wrist to the middle knuckle; little knuckle to the index's
    let (direction, across) = (p[9] - p[0], p[5] - p[17]);
    let palm = if left { direction.cross(across) } else { across.cross(direction) };
    let joints: [[Vec3; 4]; 5] = std::array::from_fn(|f| std::array::from_fn(|j| p[1 + f * 4 + j]));
    (crate::ik::hand_turn(left, direction, palm), super::fingers::from_joints(left, &joints, direction, palm))
}

/// Stops the helper, and what it started: a virtual environment's
/// python.exe is only a launcher for the real one, which would stay, holding
/// the camera
fn stop(child: &mut std::process::Child) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).creation_flags(0x0800_0000).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether `pick` lets MediaPipe drive a part
fn may(pick: Pick) -> bool {
    matches!(pick, Pick::Auto | Pick::Only(Source::MediaPipe))
}

#[cfg(target_os = "windows")]
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut std::ffi::c_void, model: u32) -> i32;
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn restart(config: &Config) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !config.mediapipe {
        return;
    }
    let config = config.clone();
    let _ = std::thread::Builder::new().name("theater-puppet-mediapipe".into()).spawn(move || {
        let alive = || GENERATION.load(Ordering::SeqCst) == generation;
        let wait = |seconds: u64| {
            for _ in 0..seconds * 5 {
                if !alive() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        };
        let status = |text: String| super::set_status(Source::MediaPipe, text);
        if config.camera.is_empty() {
            return status("pick its camera below".into());
        }
        // Give the last helper time to let go of the camera
        wait(1);
        // The camera's number: its place among the PC's cameras (OBS's
        // camera source may not be loaded yet when OBS starts)
        unsafe { CoInitializeEx(std::ptr::null_mut(), 0) };
        let mut index = None;
        for _ in 0..30 {
            index = super::menu::cameras().iter().position(|(_, id)| *id == config.camera);
            if index.is_some() || !alive() {
                break;
            }
            wait(1);
        }
        let name = config.camera.split(':').next().unwrap_or("").to_string();
        let Some(index) = index else {
            crate::log(&format!("[Puppet] ⚠️ MediaPipe: the camera \"{name}\" isn't on this PC now"));
            return status(format!("the camera \"{name}\" isn't plugged in"));
        };
        let Some(script) = crate::data_dir().map(|d| d.join("mediapipe").join("puppet_mediapipe.py")).filter(|p| p.is_file()) else {
            crate::log("[Puppet] ⚠️ MediaPipe: its helper (data/mediapipe/puppet_mediapipe.py) is missing");
            return status("its helper is missing: reinstall Theater Puppet".into());
        };
        let python = if config.python.trim().is_empty() { "python".to_string() } else { config.python.trim().to_string() };
        let a = &config.assign;
        let face = may(a.expressions) || may(a.head_turn) || may(a.head_position);
        let pose = may(a.arms) || may(a.lower_body);
        let hands = may(a.fingers);
        if !(face || pose || hands) {
            return status("nothing is set to follow it (What tracks what)".into());
        }
        let mut told = String::new();
        while alive() {
            let mut command = Command::new(&python);
            command.arg("-u").arg(&script).arg("--camera").arg(index.to_string()).arg("--models").arg(crate::config_dir().join("models")).arg("--parent").arg(std::process::id().to_string());
            for (on, flag) in [(face, "--face"), (pose, "--pose"), (hands, "--hands")] {
                if on {
                    command.arg(flag);
                }
            }
            command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                // No console window
                command.creation_flags(0x0800_0000);
            }
            let mut child = match command.spawn() {
                Ok(c) => c,
                Err(e) => {
                    crate::log(&format!("[Puppet] ⚠️ MediaPipe: can't start \"{python}\": {e}"));
                    return status(format!("can't start Python (\"{python}\"): set its path below"));
                }
            };
            crate::log(&format!("[Puppet] 📷 MediaPipe: started on camera {index} (\"{name}\") with {python}"));
            status("starting".into());
            // What it last complained of
            let complaint = Arc::new(Mutex::new(String::new()));
            if let Some(err) = child.stderr.take() {
                let complaint = complaint.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(err).lines().map_while(Result::ok) {
                        if line.contains("Error") || line.contains("error") {
                            *complaint.lock().unwrap_or_else(|e| e.into_inner()) = line;
                        }
                    }
                });
            }
            let out = child.stdout.take();
            let height = config.user_height;
            let reader = std::thread::spawn(move || {
                let Some(out) = out else { return };
                let mut mapper = Mapper::new(height);
                let (mut fps, mut received) = (0.0, false);
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if GENERATION.load(Ordering::SeqCst) != generation {
                        break;
                    }
                    let seen = mapper.line(&line);
                    if let Some(e) = seen.error {
                        crate::log(&format!("[Puppet] ⚠️ MediaPipe: {e}"));
                        super::set_status(Source::MediaPipe, e);
                        continue;
                    }
                    if let Some(s) = seen.status {
                        super::set_status(Source::MediaPipe, s);
                        continue;
                    }
                    fps = seen.fps.unwrap_or(fps);
                    let what: Vec<&str> = [(seen.face.is_some(), "face"), (seen.body.is_some(), "body"), (seen.hands.is_some(), "hands")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
                    super::set_status(Source::MediaPipe, if what.is_empty() { "camera on, nobody in view".to_string() } else { format!("receiving {} ({fps:.0} a second)", what.join(", ")) });
                    if !received && !what.is_empty() {
                        received = true;
                        crate::log(&format!("[Puppet] 📷 MediaPipe: receiving {}", what.join(", ")));
                    }
                    if let Some(face) = seen.face {
                        super::set_face(Source::MediaPipe, face);
                    }
                    if let Some(body) = seen.body {
                        super::set_body(Source::MediaPipe, body);
                    }
                    if let Some(hands) = seen.hands {
                        super::set_hands(Source::MediaPipe, hands);
                    }
                }
            });
            // Until it's turned off, or the helper stops by itself
            let stopped = loop {
                if !alive() {
                    stop(&mut child);
                    break None;
                }
                if let Ok(Some(code)) = child.try_wait() {
                    break Some(code);
                }
                std::thread::sleep(Duration::from_millis(200));
            };
            let _ = reader.join();
            let Some(code) = stopped else { return };
            let why = complaint.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let line = format!("[Puppet] ⚠️ MediaPipe: its helper stopped ({code}) {why}");
            if line != told {
                crate::log(&line);
                told = line;
            }
            // (its own word for why, when it gave one, is already the status)
            if !matches!(code.code(), Some(2..=5)) {
                status(format!("its helper stopped: {}", if why.is_empty() { "see theater_puppet.log" } else { &why }));
            }
            wait(5);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::arkit_index;

    /// 33 body landmarks as the helper writes them: a user facing the
    /// camera, the left hand raised in front of the left shoulder
    fn pose_json() -> String {
        let mut world = vec![[0.0f32, 0.0, 0.0, 0.1]; 33];
        // (X right in the picture: your left; Y down; Z away from the camera)
        world[11] = [0.18, -0.50, 0.0, 0.99];
        world[12] = [-0.18, -0.50, 0.0, 0.99];
        world[13] = [0.25, -0.30, -0.10, 0.9];
        world[15] = [0.20, -0.45, -0.30, 0.9];
        // The hand upright, its palm to the camera: the fingers up, the index nearer the middle
        world[19] = [0.17, -0.55, -0.30, 0.9];
        world[17] = [0.23, -0.55, -0.30, 0.9];
        let rows: Vec<String> = world.iter().map(|w| format!("[{},{},{},{}]", w[0], w[1], w[2], w[3])).collect();
        format!(r#"{{"pose":{{"world":[{}],"wrists":[[0.7,0.4],[0.3,0.9]]}}}}"#, rows.join(","))
    }

    #[test]
    fn a_face_line_is_read() {
        let mut mapper = Mapper::new(1.7);
        // Looking straight at the camera, 50 cm away
        let straight = r#"{"face":{"shapes":{"_neutral":0.0,"jawOpen":0.4,"eyeBlinkLeft":0.58},"matrix":[1,0,0,0, 0,1,0,0, 0,0,1,-50, 0,0,0,1]}}"#;
        let seen = mapper.line(straight);
        let face = seen.face.unwrap();
        assert_eq!(face.shapes[arkit_index("jawOpen").unwrap()], 0.4);
        assert_eq!(face.shapes[arkit_index("eyeBlinkLeft").unwrap()], 1.0);
        assert!(face.head.angle_between(Quat::IDENTITY) < 1e-3);
        let head = seen.body.unwrap().head.unwrap();
        assert!((head.position - Vec3::new(0.0, 1.7 * 0.93, 0.0)).length() < 1e-4);

        // The head turned to your left (the picture's right), 10 cm to your left
        let (s, c) = 30f32.to_radians().sin_cos();
        let turned = format!(r#"{{"face":{{"shapes":{{"jawOpen":0.0}},"matrix":[{c},0,{s},10, 0,1,0,0, {},0,{c},-50, 0,0,0,1]}}}}"#, -s);
        let seen = mapper.line(&turned);
        let nose = seen.face.unwrap().head * Vec3::Z;
        assert!(nose.x > 0.45 && nose.z > 0.8, "{nose:?}");
        let head = seen.body.unwrap().head.unwrap();
        assert!((head.position.x - 0.10).abs() < 1e-4, "{:?}", head.position);
    }

    #[test]
    fn a_raised_hand_is_the_avatars() {
        let mut mapper = Mapper::new(1.7);
        let seen = mapper.line(&pose_json());
        let body = seen.body.unwrap();
        let hand = body.left_hand.unwrap();
        // In front of the left shoulder (+X, +Z), just under the shoulders' height
        assert!(hand.position.x > 0.15 && hand.position.z > 0.25, "{:?}", hand.position);
        assert!((hand.position.y - (1.7 * 0.82 - 0.05)).abs() < 1e-3, "{:?}", hand.position);
        // Fingers up, the palm toward the camera
        let (fingers, palm) = (hand.rotation * Vec3::X, hand.rotation * Vec3::NEG_Y);
        assert!(fingers.y > 0.95 && palm.z > 0.95, "{fingers:?} {palm:?}");
        assert!(body.left_elbow.is_some());
        // The right arm isn't in view: left to the idle pose
        assert!(body.right_hand.is_none() && body.left_foot.is_none());
    }

    #[test]
    fn a_hand_goes_to_the_wrist_it_is_on() {
        // A flat hand: fingers up, the palm to the camera, the thumb toward the picture's left
        let mut points = vec![[0.0f32, 0.0, 0.0]; 21];
        for f in 0..5 {
            let x = -0.04 + 0.02 * f as f32;
            for j in 0..4 {
                points[1 + f * 4 + j] = [x, -0.05 - 0.02 * j as f32, 0.0];
            }
        }
        let rows: Vec<String> = points.iter().map(|p| format!("[{},{},{}]", p[0], p[1], p[2])).collect();
        // Called "Right" (as in a mirror), and at the body's left wrist
        let pose = pose_json();
        let line = format!(r#"{{"hands":[{{"world":[{}],"wrist":[0.71,0.41],"label":"Right"}}],{}"#, rows.join(","), &pose[1..]);
        let seen = Mapper::new(1.7).line(&line);
        let hands = seen.hands.unwrap();
        assert!(hands.left.is_some() && hands.right.is_none());
        let fingers = hands.left.unwrap();
        assert!(fingers.bend[1..].iter().flatten().all(|b| b.abs() < 0.01), "{:?}", fingers.bend);
        // The hand model's turn is the arm's hand's
        let hand = seen.body.unwrap().left_hand.unwrap();
        let (tips, palm) = (hand.rotation * Vec3::X, hand.rotation * Vec3::NEG_Y);
        assert!(tips.y > 0.95 && palm.z > 0.95, "{tips:?} {palm:?}");

        // Without the body: the hand model's word, the other way round
        let alone = format!(r#"{{"hands":[{{"world":[{}],"wrist":[0.71,0.41],"label":"Left"}}]}}"#, rows.join(","));
        let seen = Mapper::new(1.7).line(&alone);
        assert!(seen.hands.unwrap().right.is_some() && seen.body.is_none());
    }

    #[test]
    fn its_other_lines() {
        let mut mapper = Mapper::new(1.7);
        assert_eq!(mapper.line(r#"{"status":"opening camera 0"}"#).status.as_deref(), Some("opening camera 0"));
        assert!(mapper.line(r#"{"error":"camera 0 didn't open"}"#).error.is_some());
        let nothing = mapper.line("{}");
        assert!(nothing.face.is_none() && nothing.body.is_none() && nothing.hands.is_none());
        assert!(mapper.line("Traceback").face.is_none());
    }
}
