//! Tracking's settings (plugin_config/theater-puppet/tracking.json): which
//! inputs are on, where they are, and which input drives which part of the
//! avatar. Edited in the Tracking window; the inputs restart when it changes.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::{Part, Pick};

/// A phone's face app: where it is on the network
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Phone {
    pub enabled: bool,
    /// The phone's address (the app shows it), to ask it for its data
    pub ip: String,
    pub port: u16,
}

impl Default for Phone {
    fn default() -> Self {
        Phone { enabled: false, ip: String::new(), port: 0 }
    }
}

/// Which input drives each part of the avatar
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Assign {
    pub expressions: Pick,
    pub head_turn: Pick,
    pub head_position: Pick,
    pub arms: Pick,
    pub fingers: Pick,
    pub lower_body: Pick,
}

impl Assign {
    pub fn get(&self, part: Part) -> Pick {
        match part {
            Part::Expressions => self.expressions,
            Part::HeadTurn => self.head_turn,
            Part::HeadPosition => self.head_position,
            Part::Arms => self.arms,
            Part::Fingers => self.fingers,
            Part::LowerBody => self.lower_body,
        }
    }

    pub fn set(&mut self, part: Part, pick: Pick) {
        match part {
            Part::Expressions => self.expressions = pick,
            Part::HeadTurn => self.head_turn = pick,
            Part::HeadPosition => self.head_position = pick,
            Part::Arms => self.arms = pick,
            Part::Fingers => self.fingers = pick,
            Part::LowerBody => self.lower_body = pick,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ifacialmocap: Phone,
    pub meowface: Phone,
    pub steamvr: bool,
    /// The headset's own telemetry server: its address, poses on this port, fingers on the next
    pub telemetry: Phone,
    /// VR telemetry's hands, hips and feet moved this much farther from the head (1 = as tracked)
    pub telemetry_scale: f32,
    pub leap: bool,
    /// The Leap is on the headset (looking where you look), not on the desk
    pub leap_on_head: bool,
    /// The Leap's hands raised (+) or lowered (-) on the avatar, metres
    pub leap_height: f32,
    /// The Leap's hands moved forward (+) or back (-) on the avatar, metres
    pub leap_forward: f32,
    pub mediapipe: bool,
    /// MediaPipe's camera: its device id, as OBS's Video Capture Device lists it
    pub camera: String,
    /// A Python of your own that has mediapipe and opencv (only in the file;
    /// empty: Puppet's own, which it installs by itself the first time)
    pub python: String,
    pub assign: Assign,
    /// As in a mirror: your left moves the avatar's right
    pub mirror: bool,
    /// How much of your head's turn the avatar's makes (1 = all)
    pub head_strength: f32,
    /// How much of a lean the spine and hips take before the legs bend
    /// (0 = the legs only, 1 = all they can)
    pub spine_lean: f32,
    /// How much of the head's turn the torso makes with it (0 = none)
    pub torso_follow: f32,
    /// 0 = raw, 1 = very smooth (and a little late)
    pub smoothing: f32,
    /// Your height in metres: what's tracked is scaled from yours to the avatar's
    pub user_height: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ifacialmocap: Phone { enabled: false, ip: String::new(), port: 49983 },
            meowface: Phone { enabled: false, ip: String::new(), port: 21412 },
            steamvr: false,
            telemetry: Phone { enabled: false, ip: String::new(), port: super::telemetry::POSE_PORT },
            telemetry_scale: 1.0,
            leap: false,
            leap_on_head: false,
            leap_height: 0.0,
            leap_forward: 0.0,
            mediapipe: false,
            camera: String::new(),
            python: String::new(),
            assign: Assign::default(),
            mirror: false,
            head_strength: 1.0,
            spine_lean: 1.0,
            torso_follow: 0.25,
            smoothing: 0.5,
            user_height: 1.7,
        }
    }
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

fn file() -> std::path::PathBuf {
    crate::config_dir().join("tracking.json")
}

/// The settings (read from the file the first time)
pub fn get() -> Config {
    let mut cached = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    cached
        .get_or_insert_with(|| {
            if cfg!(test) {
                return Config::default();
            }
            std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
        })
        .clone()
}

/// The settings that only change how what's tracked is used, read every
/// frame: a change of them alone doesn't restart the inputs
fn without_live(c: &Config) -> Config {
    Config { leap_height: 0.0, leap_forward: 0.0, telemetry_scale: 0.0, mirror: false, head_strength: 0.0, spine_lean: 0.0, torso_follow: 0.0, smoothing: 0.0, ..c.clone() }
}

/// New settings: kept, saved, and the inputs restarted with them (when
/// more than the live settings changed)
pub fn set(config: Config) {
    let restart = {
        let mut cached = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
        if cached.as_ref() == Some(&config) {
            return;
        }
        let restart = cached.as_ref().is_none_or(|old| without_live(old) != without_live(&config));
        *cached = Some(config.clone());
        restart
    };
    if !cfg!(test) {
        if let Ok(json) = serde_json::to_string_pretty(&config) {
            let _ = std::fs::write(file(), json);
        }
        if restart {
            super::start();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::Source;

    #[test]
    fn settings_survive_the_file() {
        let mut c = Config::default();
        c.ifacialmocap = Phone { enabled: true, ip: "192.168.1.20".into(), port: 49983 };
        c.assign.arms = Pick::Only(Source::Leap);
        c.assign.fingers = Pick::Off;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&json).unwrap(), c);
        // An older file with fewer settings still reads: the rest are the defaults
        let old: Config = serde_json::from_str(r#"{"steamvr":true}"#).unwrap();
        assert!(old.steamvr && old.meowface.port == 21412 && old.assign.arms == Pick::Auto);
    }
}
