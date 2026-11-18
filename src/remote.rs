//! Puppet driven from outside: Theater Control Panel's puppet module
//! (control-panel/modules/puppet.tcp) and Theater Scene's Puppet redeems, through one OBS
//! procedure:
//!
//! `theater_puppet_command(in string args, out string json)`, `args` being
//! JSON with an `action`. `source` names a Puppet ("" or none: every one);
//! `glb` is which of its GLBs: 0 the avatar, 1–3 its "Also draw" ones, or a
//! file's name (without .glb).
//!
//! - `list`: every Puppet, its GLBs (name, shape keys, shown), animations,
//!   colour slots;
//! - `animation` {clip, mode: "once" | "loop" | "stop"} ("loop" with no clip:
//!   back to the idle motion);
//! - `shape` {glb, key, state: "on" | "off" | "pingpong", value (0–1, on's
//!   and ping-pong's top), period (seconds, ping-pong's)};
//! - `shape_anim` {glb, key, from, to (0–1), duration (s), type: "up" |
//!   "down" | "pingpong", show_at_start, hide_at_end}: the key goes from
//!   `from` to `to` (up), back (down) or there and back (ping-pong), once;
//!   then it's left alone. The GLB can be shown as it starts, hidden as it ends;
//! - `glb` {glb, visible: true | false | "toggle"} (saved; showing one of a
//!   group hides the others);
//! - `group` {group, glb: a GLB of it | "next" | "none"}: shows that one of
//!   the group (the others hidden), the next one, or none;
//! - `color` {slot, color: "#rrggbb" | "reset"}: that colour slot on every
//!   GLB of the Puppet (saved in the source's settings).
//!
//! It answers `{"ok", "error"?, "puppets"?}`.

use std::ffi::CStr;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::obs::{self, Ptr};
use crate::parts::{Calldata, get_string, set_string};

/// How a shape key is driven, over what's tracked and animated
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drive {
    /// Held at a value
    Hold(f32),
    /// Back and forth from 0 to `top` every `period` seconds, until turned off
    PingPong { top: f32, period: f32 },
    /// Once, over `duration` seconds
    Once { from: f32, to: f32, duration: f32, shape: Shape, hide_at_end: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Up,
    Down,
    PingPong,
}

/// A shape key of one GLB, driven
#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub layer: usize,
    pub key: String,
    pub drive: Drive,
    /// Seconds since it began
    pub time: f32,
}

impl Override {
    /// Its value now; None once a `Once` has ended
    pub fn value(&self) -> Option<f32> {
        Some(match self.drive {
            Drive::Hold(v) => v,
            Drive::PingPong { top, period } => {
                let p = period.max(0.05);
                let phase = (self.time / p).fract();
                top * (1.0 - (phase * 2.0 - 1.0).abs())
            }
            Drive::Once { from, to, duration, shape, .. } => {
                if self.time > duration {
                    return None;
                }
                let t = (self.time / duration.max(1e-3)).clamp(0.0, 1.0);
                let k = match shape {
                    Shape::Up => t,
                    Shape::Down => 1.0 - t,
                    Shape::PingPong => 1.0 - (t * 2.0 - 1.0).abs(),
                };
                from + (to - from) * k
            }
        })
    }
}

/// One GLB of a Puppet, as the list shows it
#[derive(Clone, Debug, Default)]
pub struct GlbInfo {
    pub name: String,
    pub keys: Vec<String>,
}

/// GLB slots of a source: 0 the avatar, 1–3 its "Also draw" ones
pub const SLOTS: usize = 4;

/// Which slots show, the setting `shown` read with the group rule: of a
/// group's slots set to show, the first does
pub fn effective(shown: [bool; SLOTS], groups: &[String; SLOTS]) -> [bool; SLOTS] {
    let mut out = shown;
    for s in 0..SLOTS {
        let g = groups[s].trim();
        if out[s] && !g.is_empty() && (0..s).any(|e| out[e] && groups[e].trim().eq_ignore_ascii_case(g)) {
            out[s] = false;
        }
    }
    out
}

/// The slots' settings once slot `slot` is shown (`on`) or hidden: showing
/// one of a group hides the others
pub fn showing(shown: [bool; SLOTS], groups: &[String; SLOTS], slot: usize, on: bool) -> [bool; SLOTS] {
    let mut out = effective(shown, groups);
    out[slot] = on;
    let g = groups[slot].trim();
    if on && !g.is_empty() {
        for s in (0..SLOTS).filter(|&s| s != slot && groups[s].trim().eq_ignore_ascii_case(g)) {
            out[s] = false;
        }
    }
    out
}

/// What's asked of one Puppet, and what it says back
#[derive(Default)]
pub struct Remote {
    pub animation: crate::animation::Control,
    /// Shape key requests (layer, key, Some(drive) or None: let go), taken by the rig
    pub shapes: Vec<(usize, String, Option<Drive>)>,
    /// Hidden GLBs, by layer (missing: shown): from `shown` and `groups`
    pub hidden: Vec<bool>,
    /// Each slot's Show setting, and its group ("" none): the source's settings
    pub shown: [bool; SLOTS],
    pub groups: [String; SLOTS],
    /// Each layer's slot (a slot whose file didn't load has no layer)
    pub layer_slot: Vec<usize>,
    /// Its GLBs (the avatar first), filled in by the source
    pub glbs: Vec<GlbInfo>,
    /// Its colour slots
    pub slots: Vec<u32>,
}

impl Remote {
    pub fn is_hidden(&self, layer: usize) -> bool {
        self.hidden.get(layer).copied().unwrap_or(false)
    }

    /// Layer `layer`'s slot
    pub fn slot(&self, layer: usize) -> usize {
        self.layer_slot.get(layer).copied().unwrap_or(layer).min(SLOTS - 1)
    }

    /// The Show settings once group `group` shows `pick`: one of its GLBs (a
    /// number or a name), "next" (or nothing), or "none"
    pub fn in_group(&self, group: &str, pick: Option<&Value>) -> Result<[bool; SLOTS], String> {
        // The group's GLBs (layers), in order
        let members: Vec<usize> = (0..self.glbs.len()).filter(|&l| !group.is_empty() && self.groups[self.slot(l)].trim().eq_ignore_ascii_case(group.trim())).collect();
        if members.is_empty() {
            return Err(format!("no GLB in group \"{group}\""));
        }
        let current = effective(self.shown, &self.groups);
        let word = pick.and_then(|v| v.as_str()).unwrap_or("").trim().to_ascii_lowercase();
        Ok(match word.as_str() {
            "none" => {
                let mut out = current;
                for &l in &members {
                    out[self.slot(l)] = false;
                }
                out
            }
            "next" | "" => {
                let now = members.iter().position(|&l| current[self.slot(l)]);
                let next = members[now.map_or(0, |i| (i + 1) % members.len())];
                showing(self.shown, &self.groups, self.slot(next), true)
            }
            _ => {
                let layer = self.layer(pick)?;
                if !members.contains(&layer) {
                    return Err(format!("GLB {} isn't in group \"{group}\"", self.glbs[layer].name));
                }
                showing(self.shown, &self.groups, self.slot(layer), true)
            }
        })
    }

    /// The layers hidden, from the slots' settings
    pub fn update_hidden(&mut self) {
        let shown = effective(self.shown, &self.groups);
        self.hidden = (0..self.layer_slot.len().max(1)).map(|l| !shown[self.slot(l)]).collect();
    }

    /// `glb` (a number, or a GLB's name) as its layer
    fn layer(&self, glb: Option<&Value>) -> Result<usize, String> {
        match glb {
            None | Some(Value::Null) => Ok(0),
            Some(Value::Number(n)) => {
                let i = n.as_u64().unwrap_or(u64::MAX) as usize;
                if i < self.glbs.len().max(1) { Ok(i) } else { Err(format!("no GLB {i}")) }
            }
            Some(Value::String(s)) if s.trim().is_empty() => Ok(0),
            Some(Value::String(s)) => {
                if let Ok(i) = s.trim().parse::<usize>() {
                    return self.layer(Some(&json!(i)));
                }
                self.glbs.iter().position(|g| g.name.eq_ignore_ascii_case(s.trim())).ok_or_else(|| format!("no GLB \"{s}\""))
            }
            Some(other) => Err(format!("glb: {other}?")),
        }
    }
}

pub type Shared = Arc<Mutex<Remote>>;

/// Every Puppet source (OBS's pointer) and its remote
static SOURCES: Mutex<Vec<(usize, Shared)>> = Mutex::new(Vec::new());

pub fn add_source(source: Ptr, remote: Shared) {
    SOURCES.lock().unwrap().push((source as usize, remote));
}

pub fn remove_source(source: Ptr) {
    SOURCES.lock().unwrap().retain(|(s, _)| *s != source as usize);
}

const DECL: &CStr = c"void theater_puppet_command(in string args, out string json)";

/// Registers the procedure. Call from obs_module_load.
pub fn register() {
    let api = obs::api();
    unsafe {
        let handler = (api.obs_get_proc_handler)();
        if !handler.is_null() {
            (api.proc_handler_add)(handler, DECL.as_ptr(), command_proc, std::ptr::null_mut());
        }
    }
}

unsafe extern "C" fn command_proc(_data: Ptr, cd: *mut Calldata) {
    let args = get_string(cd, c"args");
    let json = std::panic::catch_unwind(|| answer(&args)).unwrap_or_else(|_| json!({ "ok": false, "error": "Puppet failed" }).to_string());
    set_string(cd, c"json", &json);
}

/// "#rrggbb" (or "rrggbb") as sRGB 0–1
pub fn parse_color(s: &str) -> Option<[f32; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|b| b as f32 / 255.0);
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// A number setting, clamped
fn number(args: &Value, key: &str, default: f32, lo: f32, hi: f32) -> f32 {
    args.get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .unwrap_or(default)
        .clamp(lo, hi)
}

fn flag(args: &Value, key: &str) -> bool {
    match args.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "on" | "yes" | "1"),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        _ => false,
    }
}

/// What `args` asks of one Puppet (`source`, its OBS source), done
fn apply(args: &Value, source: Ptr, remote: &Shared) -> Result<(), String> {
    let mut shown = None;
    let result = ask(args, source, remote, &mut shown);
    // Written once the remote is let go (the source's update takes it)
    if let Some(shown) = shown {
        crate::source::set_shown(source, shown);
    }
    result
}

/// `apply`'s work; `shown`: the slots' Show settings to write
fn ask(args: &Value, source: Ptr, remote: &Shared, shown: &mut Option<[bool; SLOTS]>) -> Result<(), String> {
    let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let action = text("action");
    let mut r = remote.lock().unwrap();
    match action.as_str() {
        "animation" => {
            let clip = text("clip");
            match text("mode").as_str() {
                "stop" => r.animation.stop = true,
                "loop" => {
                    if !clip.is_empty() && !r.animation.clips.contains(&clip) {
                        return Err(format!("no animation \"{clip}\""));
                    }
                    drop(r);
                    // The source's setting: saved, and its properties agree
                    crate::source::set_string_setting(source, crate::source::S_ANIMATION, &clip);
                }
                _ => {
                    if !r.animation.clips.contains(&clip) {
                        return Err(format!("no animation \"{clip}\""));
                    }
                    r.animation.play = Some(clip);
                }
            }
        }
        "shape" | "shape_anim" => {
            let layer = r.layer(args.get("glb"))?;
            let key = text("key");
            if !r.glbs.get(layer).is_some_and(|g| g.keys.iter().any(|k| k.eq_ignore_ascii_case(&key))) {
                return Err(format!("GLB {layer} has no shape key \"{key}\""));
            }
            let drive = if action == "shape" {
                match text("state").as_str() {
                    "off" => None,
                    "pingpong" | "ping-pong" | "ping pong" => {
                        Some(Drive::PingPong { top: number(args, "value", 1.0, 0.0, 1.0), period: number(args, "period", 1.0, 0.05, 600.0) })
                    }
                    _ => Some(Drive::Hold(number(args, "value", 1.0, 0.0, 1.0))),
                }
            } else {
                let shape = match text("type").as_str() {
                    "down" | "decrease" | "décroissant" | "decroissant" => Shape::Down,
                    "pingpong" | "ping-pong" | "ping pong" => Shape::PingPong,
                    _ => Shape::Up,
                };
                if flag(args, "show_at_start") {
                    *shown = Some(showing(r.shown, &r.groups, r.slot(layer), true));
                }
                Some(Drive::Once {
                    from: number(args, "from", 0.0, 0.0, 1.0),
                    to: number(args, "to", 1.0, 0.0, 1.0),
                    duration: number(args, "duration", 1.0, 0.0, 600.0),
                    shape,
                    hide_at_end: flag(args, "hide_at_end"),
                })
            };
            r.shapes.push((layer, key, drive));
        }
        "glb" => {
            let layer = r.layer(args.get("glb"))?;
            let on = match args.get("visible") {
                Some(Value::String(s)) if s.eq_ignore_ascii_case("toggle") => r.is_hidden(layer),
                _ => flag(args, "visible"),
            };
            *shown = Some(showing(r.shown, &r.groups, r.slot(layer), on));
        }
        "group" => *shown = Some(r.in_group(&text("group"), args.get("glb"))?),
        "color" => {
            let slot = args.get("slot").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).ok_or("no slot")? as u32;
            let color = text("color");
            drop(r);
            let rgb = if color.eq_ignore_ascii_case("reset") { None } else { Some(parse_color(&color).ok_or_else(|| format!("colour \"{color}\": #rrggbb or reset"))?) };
            crate::source::set_color_setting(source, slot, rgb);
        }
        "list" | "" => {}
        other => return Err(format!("no action \"{other}\"")),
    }
    Ok(())
}

pub fn answer(args: &str) -> String {
    let args: Value = serde_json::from_str(args).unwrap_or(json!({}));
    let which = args.get("source").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let names = crate::parts::sources();
    let all = SOURCES.lock().unwrap().clone();
    let puppets: Vec<(Ptr, String, Shared)> = all
        .into_iter()
        .filter_map(|(s, r)| names.iter().find(|(p, _)| *p as usize == s).map(|(p, n)| (*p, n.clone(), r)))
        .collect();
    let targets: Vec<&(Ptr, String, Shared)> = puppets.iter().filter(|(_, n, _)| which.is_empty() || *n == which).collect();
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
    let mut errors = Vec::new();
    if targets.is_empty() && action != "list" {
        errors.push(if which.is_empty() { "no Puppet source".to_string() } else { format!("no Puppet \"{which}\"") });
    }
    for (source, name, remote) in &targets {
        if let Err(e) = apply(&args, *source, remote) {
            errors.push(if targets.len() > 1 { format!("{name}: {e}") } else { e });
        }
    }
    let mut out = json!({ "ok": errors.is_empty() });
    if !errors.is_empty() {
        out["error"] = json!(errors.join("; "));
    }
    if action == "list" {
        out["puppets"] = puppets
            .iter()
            .map(|(_, name, remote)| {
                let r = remote.lock().unwrap();
                // The groups its loaded GLBs are in
                let mut groups: Vec<String> = (0..r.glbs.len()).map(|l| r.groups[r.slot(l)].trim().to_string()).filter(|g| !g.is_empty()).collect();
                groups.sort();
                groups.dedup();
                json!({
                    "source": name,
                    "glbs": r.glbs.iter().enumerate().map(|(i, g)| json!({ "index": i, "name": g.name, "keys": g.keys, "visible": !r.is_hidden(i), "group": r.groups[r.slot(i)] })).collect::<Vec<_>>(),
                    "groups": groups,
                    "animations": r.animation.clips,
                    "looping": r.animation.loop_clip,
                    "playing": r.animation.playing,
                    "colour_slots": r.slots,
                })
            })
            .collect();
    }
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(drive: Drive, time: f32) -> Option<f32> {
        Override { layer: 0, key: "k".into(), drive, time }.value()
    }

    #[test]
    fn shape_drives() {
        assert_eq!(at(Drive::Hold(0.7), 5.0), Some(0.7));
        let pp = Drive::PingPong { top: 1.0, period: 2.0 };
        assert_eq!(at(pp, 0.0), Some(0.0));
        assert_eq!(at(pp, 1.0), Some(1.0));
        assert_eq!(at(pp, 1.5), Some(0.5));
        let once = |shape| Drive::Once { from: 0.2, to: 0.8, duration: 2.0, shape, hide_at_end: false };
        assert_eq!(at(once(Shape::Up), 0.0), Some(0.2));
        assert!((at(once(Shape::Up), 1.0).unwrap() - 0.5).abs() < 1e-6);
        assert!((at(once(Shape::Down), 0.5).unwrap() - 0.65).abs() < 1e-6);
        assert!((at(once(Shape::PingPong), 1.0).unwrap() - 0.8).abs() < 1e-6);
        assert!((at(once(Shape::PingPong), 2.0).unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(at(once(Shape::Up), 2.1), None);
    }

    #[test]
    fn glbs_are_found_by_number_or_name() {
        let r = Remote { glbs: vec![GlbInfo { name: "Armature".into(), keys: vec![] }, GlbInfo { name: "Hat".into(), keys: vec![] }], ..Default::default() };
        assert_eq!(r.layer(None), Ok(0));
        assert_eq!(r.layer(Some(&json!(1))), Ok(1));
        assert_eq!(r.layer(Some(&json!("hat"))), Ok(1));
        assert_eq!(r.layer(Some(&json!("1"))), Ok(1));
        assert!(r.layer(Some(&json!(2))).is_err());
        assert!(r.layer(Some(&json!("nope"))).is_err());
    }

    #[test]
    fn one_glb_of_a_group_shows() {
        let groups = ["".to_string(), "hats".to_string(), "hats".to_string(), "".to_string()];
        // Both hats set to show: the first does
        assert_eq!(effective([true, true, true, true], &groups), [true, true, false, true]);
        // Showing the second hat hides the first
        assert_eq!(showing([true, true, false, true], &groups, 2, true), [true, false, true, true]);
        // Hiding one leaves the rest
        assert_eq!(showing([true, true, false, true], &groups, 1, false), [true, false, false, true]);
        // No group: nothing else changes
        assert_eq!(showing([true, true, false, false], &groups, 3, true), [true, true, false, true]);
    }

    #[test]
    fn a_group_shows_a_name_the_next_one_or_none() {
        // The avatar, then hats in Also draw 1 and 3 (2 didn't load)
        let glb = |n: &str| GlbInfo { name: n.into(), keys: vec![] };
        let mut r = Remote {
            glbs: vec![glb("Armature"), glb("Cap"), glb("Crown")],
            layer_slot: vec![0, 1, 3],
            shown: [true, true, true, true],
            groups: ["".into(), "hats".into(), "".into(), "Hats".into()],
            ..Default::default()
        };
        r.update_hidden();
        // Both set to show: the first of the group does
        assert_eq!(r.hidden, vec![false, false, true]);
        assert_eq!(r.in_group("hats", Some(&json!("crown"))), Ok([true, false, true, true]));
        assert_eq!(r.in_group("hats", Some(&json!("next"))), Ok([true, false, true, true]));
        assert_eq!(r.in_group("hats", Some(&json!("none"))), Ok([true, false, true, false]));
        assert!(r.in_group("hats", Some(&json!("Armature"))).is_err());
        assert!(r.in_group("shoes", None).is_err());
        // From the crown, next wraps to the cap
        r.shown = [true, false, true, true];
        assert_eq!(r.in_group("HATS", None), Ok([true, true, true, false]));
    }

    #[test]
    fn colours_are_read() {
        assert_eq!(parse_color("#ff8000"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(parse_color("00FF00"), Some([0.0, 1.0, 0.0]));
        assert_eq!(parse_color("red"), None);
    }
}
