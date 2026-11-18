//! What the Puppet Tracking dock (theater-scripts/puppet_tracking_dock.rhai,
//! one of Theater Scene's Rhai docks) asks Puppet, through OBS's procedure
//! handler:
//!
//! `theater_puppet_tracking(in string args, out string json)`, `args` being
//! JSON with an `action`:
//! - `get` (or none): the state below;
//! - `set`: the settings given (`leap_height`, `leap_forward`, metres);
//! - `leap_centre`: the Leap's hands where they are now become hands resting
//!   in front of the avatar (sets `leap_height` and `leap_forward`);
//! - `telemetry_calibrate` (`now`: at once, else when both triggers are
//!   pressed), `telemetry_reset`: VR telemetry's T-pose calibration;
//! - `mediapipe_recentre`: where the face is now becomes the head at rest;
//! - `mediapipe_restart`: MediaPipe's helper started again.
//!
//! It answers `{"leap", "leap_height", "leap_forward", "leap_status",
//! "leap_hands", "mediapipe", "mediapipe_status", "done"}`.

use std::ffi::CStr;

use serde_json::{Value, json};

use super::{Source, config, leap, mediapipe, telemetry};
use crate::obs::{self, Ptr};
use crate::parts::{Calldata, get_string, set_string};

const DECL: &CStr = c"void theater_puppet_tracking(in string args, out string json)";

/// Registers the procedure. Call from obs_module_load.
pub fn register() {
    let api = obs::api();
    unsafe {
        let handler = (api.obs_get_proc_handler)();
        if !handler.is_null() {
            (api.proc_handler_add)(handler, DECL.as_ptr(), tracking_proc, std::ptr::null_mut());
        }
    }
}

unsafe extern "C" fn tracking_proc(_data: Ptr, cd: *mut Calldata) {
    let args = get_string(cd, c"args");
    let json = std::panic::catch_unwind(|| answer(&args)).unwrap_or_else(|_| "{}".into());
    set_string(cd, c"json", &json);
}

/// What the dock asked (`args`), done; the state after it, as JSON
pub fn answer(args: &str) -> String {
    let args: Value = serde_json::from_str(args).unwrap_or(json!({}));
    let number = |k: &str| args.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
    let done = match args.get("action").and_then(|a| a.as_str()).unwrap_or("get") {
        "set" => {
            let mut c = config::get();
            if let Some(v) = number("leap_height") {
                c.leap_height = v.clamp(-0.6, 0.6);
            }
            if let Some(v) = number("leap_forward") {
                c.leap_forward = v.clamp(-0.6, 0.6);
            }
            config::set(c);
            true
        }
        "leap_centre" => leap::centre(),
        "mediapipe_recentre" => {
            mediapipe::recentre();
            true
        }
        "telemetry_calibrate" => {
            telemetry::calibrate(args.get("now").and_then(|n| n.as_bool()).unwrap_or(false));
            true
        }
        "telemetry_reset" => {
            telemetry::reset_calibration();
            true
        }
        "mediapipe_restart" => {
            mediapipe::restart(&config::get());
            true
        }
        _ => true,
    };
    let c = config::get();
    let statuses = super::statuses();
    let status = |s: Source, on: bool| {
        if !on {
            return "off (turn it on in the Tracking window)".to_string();
        }
        statuses.iter().find(|(src, _, _)| *src == s).map(|(_, _, t)| t.clone()).unwrap_or_else(|| "starting".into())
    };
    json!({
        "leap": c.leap,
        "leap_height": c.leap_height,
        "leap_forward": c.leap_forward,
        "leap_status": status(Source::Leap, c.leap),
        "leap_hands": leap::hands_seen(),
        "telemetry": c.telemetry.enabled,
        "telemetry_status": status(Source::Telemetry, c.telemetry.enabled),
        "telemetry_calibration": telemetry::calibration_state(),
        "mediapipe": c.mediapipe,
        "mediapipe_status": status(Source::MediaPipe, c.mediapipe),
        "done": done,
    })
    .to_string()
}
