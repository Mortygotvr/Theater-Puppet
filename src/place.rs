//! Where each Puppet stands and which way it faces, for Theater Scene's
//! docks (theater-scripts/puppet_place_dock.rhai), through OBS's procedure
//! handler:
//!
//! `theater_puppet_place(in string args, out string json)`, `args` being JSON:
//! - `{"action": "get"}` (or none): every Puppet's place;
//! - `{"action": "set", "source": name, "turn": degrees, "x", "y", "z": metres}`:
//!   any of them, on that Puppet ("" or none = every Puppet). They are the
//!   source's own Turn and Position settings, so its properties agree, and
//!   the turn eases in as the slider's does (the hair swings after it).
//!
//! It answers `{"puppets": [{"source", "turn", "x", "y", "z"}], "done"}`.

use std::ffi::CStr;

use serde_json::{Value, json};

use crate::obs::{self, Ptr};
use crate::parts::{Calldata, get_string, set_string};
use crate::source::{S_POSITION, S_YAW};

const DECL: &CStr = c"void theater_puppet_place(in string args, out string json)";

/// Turn's range, degrees; position's, metres (x, y, z): the properties' sliders
pub const TURN: f64 = 180.0;
pub const RANGE: [f64; 3] = [3.0, 2.0, 3.0];

/// Registers the procedure. Call from obs_module_load.
pub fn register() {
    let api = obs::api();
    unsafe {
        let handler = (api.obs_get_proc_handler)();
        if !handler.is_null() {
            (api.proc_handler_add)(handler, DECL.as_ptr(), place_proc, std::ptr::null_mut());
        }
    }
}

unsafe extern "C" fn place_proc(_data: Ptr, cd: *mut Calldata) {
    let args = get_string(cd, c"args");
    let json = std::panic::catch_unwind(|| answer(&args)).unwrap_or_else(|_| "{}".into());
    set_string(cd, c"json", &json);
}

/// The settings `args` asks for, each clamped to its slider's range
pub fn changes(args: &Value) -> Vec<(&'static CStr, f64)> {
    let number = |k: &str| args.get(k).and_then(|v| v.as_f64()).filter(|v| v.is_finite());
    let mut out = Vec::new();
    if let Some(t) = number("turn") {
        // Any angle, brought into -180..180
        out.push((S_YAW, (t + TURN).rem_euclid(2.0 * TURN) - TURN));
    }
    for (i, k) in ["x", "y", "z"].into_iter().enumerate() {
        if let Some(v) = number(k) {
            out.push((S_POSITION[i], v.clamp(-RANGE[i], RANGE[i])));
        }
    }
    out
}

fn answer(args: &str) -> String {
    let args: Value = serde_json::from_str(args).unwrap_or(json!({}));
    let api = obs::api();
    let puppets = crate::parts::sources();
    let mut done = true;
    if args.get("action").and_then(|a| a.as_str()) == Some("set") {
        let which = args.get("source").and_then(|s| s.as_str()).unwrap_or("");
        let changes = changes(&args);
        let targets: Vec<Ptr> = puppets.iter().filter(|(_, n)| which.is_empty() || n == which).map(|(s, _)| *s).collect();
        done = !targets.is_empty();
        if !changes.is_empty() {
            for source in targets {
                unsafe {
                    let data = (api.obs_data_create)();
                    if data.is_null() {
                        continue;
                    }
                    for (key, value) in &changes {
                        (api.obs_data_set_double)(data, key.as_ptr(), *value);
                    }
                    // Merged into its settings, then its update runs
                    (api.obs_source_update)(source, data);
                    (api.obs_data_release)(data);
                }
            }
        }
    }
    let list: Vec<Value> = puppets
        .iter()
        .map(|(source, name)| unsafe {
            let settings = (api.obs_source_get_settings)(*source);
            let get = |k: &CStr| if settings.is_null() { 0.0 } else { (api.obs_data_get_double)(settings, k.as_ptr()) };
            let v = json!({ "source": name, "turn": get(S_YAW), "x": get(S_POSITION[0]), "y": get(S_POSITION[1]), "z": get(S_POSITION[2]) });
            if !settings.is_null() {
                (api.obs_data_release)(settings);
            }
            v
        })
        .collect();
    json!({ "puppets": list, "done": done }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_wrap_and_positions_stay_in_range() {
        let c = changes(&json!({ "turn": 270.0, "x": 10.0, "z": -0.5 }));
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], (S_YAW, -90.0));
        assert_eq!(c[1], (S_POSITION[0], 3.0));
        assert_eq!(c[2], (S_POSITION[2], -0.5));
        assert_eq!(changes(&json!({ "turn": -180.0 }))[0].1, -180.0);
        assert_eq!(changes(&json!({ "turn": 45 }))[0].1, 45.0);
        assert!(changes(&json!({ "action": "set" })).is_empty());
    }
}
