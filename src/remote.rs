//! Puppet driven from outside: Theater Control Panel's puppet module
//! (control-panel/modules/puppet.tcp) and Theater Scene's Puppet redeems, through one OBS
//! procedure:
//!
//! `theater_puppet_command(in string args, out string json)`, `args` being
//! JSON with an `action`. `source` names a Puppet ("" or none: every one);
//! `glb` is which of its GLBs: 0 the avatar, 1, 2… its "Also draw" ones, or a
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
//! - `tile` {glb, state: "on" | "off" | "toggle", name, set}: `name` is a
//!   tile the GLB has (made by Theater Rig: Make Tiles, or a VRM's
//!   expressions converted), or "next" / "random" in the set `set` (none: all
//!   its tiles). One tile per material at a time. (Old redeems' `material` +
//!   `columns` + `rows` + `cell` grid still works);
//! - `color` {slot, color: "#rrggbb" | "reset"}: that colour slot on every
//!   GLB of the Puppet (saved in the source's settings).
//!
//! `glb`, `group`, `shape_anim` (its show and hide) and `tile` take `fade`
//! (seconds, 0 at once): a GLB dissolves in or out, a tile crossfades.
//!
//! `tile` can also hold a shape key while it shows: `key` (one of that GLB's)
//! and `key_value` (0–1, 1 if not given); it's let go when the tile goes.
//!
//! Every action but `shape_anim` and `list` takes `timer` (seconds, 0 or
//! none: it stays): when it runs out, what the command changed goes back to
//! how it was (a GLB's or group's Show, a tile and its key, a shape key off,
//! a colour, an animation: the loop before, or a once stopped). A command on
//! the same thing before then takes the timer's place, still going back to
//! how it was before the first.
//!
//! - `tile_previews` {glb, size (pixels, 64 if not given)} and, for a grid,
//!   `material` + `columns` + `rows`: a picture of each tile (what it shows on
//!   each material it moves, side by side; a PNG in
//!   base64), as `previews`: [{name, png}] (a grid's names are its cells).
//!   Only the first Puppet (or `source`'s).
//!
//! It answers `{"ok", "error"?, "puppets"?, "previews"?}`.

use std::ffi::CStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    Once { from: f32, to: f32, duration: f32, shape: Shape, hide_at_end: bool, fade: f32 },
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
#[derive(Clone, Default)]
pub struct GlbInfo {
    /// The model itself (for its tiles' pictures)
    pub model: Option<Arc<crate::model::Model>>,
    pub name: String,
    pub keys: Vec<String>,
    /// Its materials' names, and its tiles
    pub materials: Vec<String>,
    pub tiles: Vec<crate::model::Tile>,
}

/// A tile showing on one GLB
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveTile {
    pub layer: usize,
    /// Its name ("" for one of a grid)
    pub name: String,
    /// Each material it moves (in its GLB) and where to
    pub binds: Vec<(usize, [f32; 4])>,
    pub weight: f32,
    /// A grid's columns, rows and cell
    pub grid: Option<(u32, u32, u32)>,
    /// A shape key held while it shows, and its value
    pub key: Option<(String, f32)>,
}

/// How a timed command is undone
#[derive(Clone, Debug)]
pub enum Undo {
    /// Another command
    Args(Value),
    /// The tiles that were on those materials of a layer (none: no tile)
    Tiles { layer: usize, materials: Vec<usize>, before: Vec<ActiveTile>, fade: f32 },
}

/// A timed command: when it's undone, and what it changed (`target`: a
/// later command on the same thing takes its place)
#[derive(Clone, Debug)]
pub struct Timer {
    pub due: Instant,
    pub target: String,
    pub undo: Undo,
}

/// Where cell `cell` of a `columns` x `rows` grid is (scale x, y, offset x, y),
/// cell 0 the top left, across then down
pub fn grid_cell(columns: u32, rows: u32, cell: u32) -> [f32; 4] {
    let (c, r) = (columns.max(1), rows.max(1));
    let cell = cell % (c * r);
    [1.0 / c as f32, 1.0 / r as f32, (cell % c) as f32 / c as f32, (cell / c) as f32 / r as f32]
}

/// A material's texture placement with the tiles on it: its own, moved
/// toward each one by its weight
pub fn tiled(base: [f32; 4], tiles: &[ActiveTile], layer: usize, material: usize) -> Option<[f32; 4]> {
    let mut st = base;
    let mut any = false;
    for t in tiles.iter().filter(|t| t.layer == layer) {
        for (m, target) in &t.binds {
            if *m == material {
                any = true;
                for k in 0..4 {
                    st[k] += (target[k] - base[k]) * t.weight;
                }
            }
        }
    }
    any.then_some(st)
}

// GLB slots of a source: 0 the avatar, 1, 2… its "Also draw" ones (as many
// as added). A slot missing from `shown` shows; one missing from `groups` has none.

fn group_of(groups: &[String], s: usize) -> &str {
    groups.get(s).map(|g| g.trim()).unwrap_or("")
}

/// Which slots show, the setting `shown` read with the group rule: of a
/// group's slots set to show, the first does
pub fn effective(shown: &[bool], groups: &[String]) -> Vec<bool> {
    let mut out = shown.to_vec();
    for s in 0..out.len() {
        let g = group_of(groups, s);
        if out[s] && !g.is_empty() && (0..s).any(|e| out[e] && group_of(groups, e).eq_ignore_ascii_case(g)) {
            out[s] = false;
        }
    }
    out
}

/// The slots' settings once slot `slot` is shown (`on`) or hidden: showing
/// one of a group hides the others
pub fn showing(shown: &[bool], groups: &[String], slot: usize, on: bool) -> Vec<bool> {
    let mut out = effective(shown, groups);
    if out.len() <= slot {
        out.resize(slot + 1, true);
    }
    out[slot] = on;
    let g = group_of(groups, slot);
    if on && !g.is_empty() {
        for s in 0..out.len() {
            if s != slot && group_of(groups, s).eq_ignore_ascii_case(g) {
                out[s] = false;
            }
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
    pub shown: Vec<bool>,
    pub groups: Vec<String>,
    /// Each layer's slot (a slot whose file didn't load has no layer)
    pub layer_slot: Vec<usize>,
    /// Its GLBs (the avatar first), filled in by the source
    pub glbs: Vec<GlbInfo>,
    /// Its colour slots
    pub slots: Vec<u32>,
    /// Tiles showing (not saved)
    pub tiles: Vec<ActiveTile>,
    /// How much of each layer shows now (0–1), easing toward its Show
    /// setting over that layer's fade seconds
    pub fade: Vec<f32>,
    pub fade_seconds: Vec<f32>,
    /// The fade the next Show change takes (a command's), then 0
    pub pending_fade: f32,
    /// Each (layer, material)'s crossfade, seconds (the last tile command's)
    pub tile_fade: Vec<((usize, usize), f32)>,
    /// Timed commands, to undo
    pub timers: Vec<Timer>,
}

impl Remote {
    pub fn is_hidden(&self, layer: usize) -> bool {
        self.hidden.get(layer).copied().unwrap_or(false)
    }

    /// Layer `layer`'s slot
    pub fn slot(&self, layer: usize) -> usize {
        self.layer_slot.get(layer).copied().unwrap_or(layer)
    }

    /// The Show settings once group `group` shows `pick`: one of its GLBs (a
    /// number or a name), "next" (or nothing), or "none"
    pub fn in_group(&self, group: &str, pick: Option<&Value>) -> Result<Vec<bool>, String> {
        // The group's GLBs (layers), in order
        let members: Vec<usize> = (0..self.glbs.len()).filter(|&l| !group.is_empty() && group_of(&self.groups, self.slot(l)).eq_ignore_ascii_case(group.trim())).collect();
        if members.is_empty() {
            return Err(format!("no GLB in group \"{group}\""));
        }
        let current = effective(&self.shown, &self.groups);
        let shows = |s: usize| current.get(s).copied().unwrap_or(true);
        let word = pick.and_then(|v| v.as_str()).unwrap_or("").trim().to_ascii_lowercase();
        Ok(match word.as_str() {
            "none" => {
                let mut out = current.clone();
                for &l in &members {
                    let s = self.slot(l);
                    if out.len() <= s {
                        out.resize(s + 1, true);
                    }
                    out[s] = false;
                }
                out
            }
            "next" | "" => {
                let now = members.iter().position(|&l| shows(self.slot(l)));
                let next = members[now.map_or(0, |i| (i + 1) % members.len())];
                showing(&self.shown, &self.groups, self.slot(next), true)
            }
            _ => {
                let layer = self.layer(pick)?;
                if !members.contains(&layer) {
                    return Err(format!("GLB {} isn't in group \"{group}\"", self.glbs[layer].name));
                }
                showing(&self.shown, &self.groups, self.slot(layer), true)
            }
        })
    }

    /// The layers hidden, from the slots' settings; a layer whose Show
    /// changed fades over `pending_fade`
    pub fn update_hidden(&mut self) {
        let shown = effective(&self.shown, &self.groups);
        let hidden: Vec<bool> = (0..self.layer_slot.len().max(1)).map(|l| !shown.get(self.slot(l)).copied().unwrap_or(true)).collect();
        let n = hidden.len();
        self.fade_seconds.resize(n, 0.0);
        for l in 0..n {
            if self.hidden.get(l) != Some(&hidden[l]) {
                self.fade_seconds[l] = self.pending_fade;
            }
            // A new layer starts as it is
            if l >= self.fade.len() {
                self.fade.push(if hidden[l] { 0.0 } else { 1.0 });
            }
        }
        self.fade.truncate(n);
        self.pending_fade = 0.0;
        self.hidden = hidden;
    }

    /// Each layer's fade `dt` seconds later
    pub fn step_fade(&mut self, dt: f32) {
        for l in 0..self.fade.len() {
            let target = if self.is_hidden(l) { 0.0 } else { 1.0 };
            let secs = self.fade_seconds.get(l).copied().unwrap_or(0.0);
            let f = &mut self.fade[l];
            *f = if secs <= 0.0 { target } else if *f < target { (*f + dt / secs).min(target) } else { (*f - dt / secs).max(target) };
        }
    }

    /// The tiles on `materials` of `layer` replaced by `add` (with their
    /// crossfade): the keys of the ones going are let go, theirs held
    fn put_tiles(&mut self, layer: usize, materials: &[usize], add: Vec<ActiveTile>, fade: f32) {
        for &m in materials {
            self.tile_fade.retain(|(k, _)| *k != (layer, m));
            self.tile_fade.push(((layer, m), fade));
        }
        let (gone, keep): (Vec<ActiveTile>, Vec<ActiveTile>) = std::mem::take(&mut self.tiles).into_iter().partition(|t| t.layer == layer && t.binds.iter().any(|(m, _)| materials.contains(m)));
        for t in &gone {
            if let Some((key, _)) = &t.key {
                self.shapes.push((t.layer, key.clone(), None));
            }
        }
        for t in &add {
            if let Some((key, v)) = &t.key {
                self.shapes.push((t.layer, key.clone(), Some(Drive::Hold(*v))));
            }
        }
        self.tiles = keep;
        self.tiles.extend(add);
    }

    /// The tiles on `materials` of `layer` now
    fn tiles_on(&self, layer: usize, materials: &[usize]) -> Vec<ActiveTile> {
        self.tiles.iter().filter(|t| t.layer == layer && t.binds.iter().any(|(m, _)| materials.contains(m))).cloned().collect()
    }

    /// The crossfade seconds of a layer's material
    pub fn tile_fade_of(&self, layer: usize, material: usize) -> f32 {
        self.tile_fade.iter().find(|(k, _)| *k == (layer, material)).map(|(_, s)| *s).unwrap_or(0.0)
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
    apply_at(args, source, remote, Instant::now())
}

fn apply_at(args: &Value, source: Ptr, remote: &Shared, now: Instant) -> Result<(), String> {
    let mut shown = None;
    let mut undo = None;
    let result = ask(args, source, remote, &mut shown, &mut undo);
    // Written once the remote is let go (the source's update takes it)
    if let Some(shown) = shown {
        crate::source::set_shown(source, shown);
    }
    if result.is_ok() {
        if let Some((target, undo)) = undo {
            let timer = number(args, "timer", 0.0, 0.0, 86_400.0);
            let mut r = remote.lock().unwrap();
            // A timer already on it: this one takes its place, undoing to
            // how it was before that one
            let before = r.timers.iter().position(|t| t.target == target).map(|i| r.timers.remove(i).undo);
            if timer > 0.0 {
                r.timers.push(Timer { due: now + Duration::from_secs_f32(timer), target, undo: before.unwrap_or(undo) });
            }
        }
    }
    result
}

/// Timed commands whose time is up, undone. Call every frame
pub fn run_due(source: Ptr, remote: &Shared) {
    run_due_at(source, remote, Instant::now());
}

fn run_due_at(source: Ptr, remote: &Shared, now: Instant) {
    let due: Vec<Timer> = {
        let mut r = remote.lock().unwrap();
        if r.timers.is_empty() {
            return;
        }
        let (due, wait) = std::mem::take(&mut r.timers).into_iter().partition(|t| t.due <= now);
        r.timers = wait;
        due
    };
    for t in due {
        match t.undo {
            Undo::Args(args) => {
                if let Err(e) = apply_at(&args, source, remote, now) {
                    crate::log(&format!("[Puppet] Timer ran out, undoing failed: {e}"));
                }
            }
            Undo::Tiles { layer, materials, before, fade } => remote.lock().unwrap().put_tiles(layer, &materials, before, fade),
        }
    }
}

/// `apply`'s work; `shown`: the slots' Show settings to write; `undo`: what
/// was changed (a name for it) and how to undo it, for a timer
fn ask(args: &Value, source: Ptr, remote: &Shared, shown: &mut Option<Vec<bool>>, undo: &mut Option<(String, Undo)>) -> Result<(), String> {
    let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let action = text("action");
    let fade = number(args, "fade", 0.0, 0.0, 60.0);
    let mut r = remote.lock().unwrap();
    if matches!(action.as_str(), "glb" | "group" | "shape_anim" | "shown") {
        r.pending_fade = fade;
    }
    // The Show settings now, to go back to
    if matches!(action.as_str(), "glb" | "group") {
        *undo = Some(("shown".into(), Undo::Args(json!({ "action": "shown", "shown": effective(&r.shown, &r.groups), "fade": fade }))));
    }
    match action.as_str() {
        "animation" => {
            let clip = text("clip");
            match text("mode").as_str() {
                "stop" => r.animation.stop = true,
                "loop" => {
                    if !clip.is_empty() && !r.animation.clips.contains(&clip) {
                        return Err(format!("no animation \"{clip}\""));
                    }
                    *undo = Some(("animation".into(), Undo::Args(json!({ "action": "animation", "mode": "loop", "clip": r.animation.loop_clip }))));
                    drop(r);
                    // The source's setting: saved, and its properties agree
                    crate::source::set_string_setting(source, crate::source::S_ANIMATION, &clip);
                }
                _ => {
                    if !r.animation.clips.contains(&clip) {
                        return Err(format!("no animation \"{clip}\""));
                    }
                    r.animation.play = Some(clip);
                    *undo = Some(("animation".into(), Undo::Args(json!({ "action": "animation", "mode": "stop" }))));
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
                    *shown = Some(showing(&r.shown, &r.groups, r.slot(layer), true));
                }
                Some(Drive::Once {
                    from: number(args, "from", 0.0, 0.0, 1.0),
                    to: number(args, "to", 1.0, 0.0, 1.0),
                    duration: number(args, "duration", 1.0, 0.0, 600.0),
                    shape,
                    hide_at_end: flag(args, "hide_at_end"),
                    fade,
                })
            };
            if action == "shape" && drive.is_some() {
                *undo = Some((format!("shape {layer} {}", key.to_ascii_lowercase()), Undo::Args(json!({ "action": "shape", "glb": layer, "key": key, "state": "off" }))));
            }
            r.shapes.push((layer, key, drive));
        }
        // A timer's undo of `glb` / `group`: the Show settings as they were
        "shown" => {
            *shown = Some(args.get("shown").and_then(|v| v.as_array()).map(|a| a.iter().map(|b| b.as_bool().unwrap_or(true)).collect()).unwrap_or_default());
        }
        "glb" => {
            let layer = r.layer(args.get("glb"))?;
            let on = match args.get("visible") {
                Some(Value::String(s)) if s.eq_ignore_ascii_case("toggle") => r.is_hidden(layer),
                _ => flag(args, "visible"),
            };
            *shown = Some(showing(&r.shown, &r.groups, r.slot(layer), on));
        }
        "group" => *shown = Some(r.in_group(&text("group"), args.get("glb"))?),
        "tile" => {
            let layer = r.layer(args.get("glb"))?;
            let state = text("state");
            let weight = number(args, "value", 1.0, 0.0, 1.0);
            let glb = r.glbs.get(layer).cloned().unwrap_or_default();
            let mut name = text("name");
            // "next" / "random": a tile of the set, from the one showing
            if matches!(name.to_ascii_lowercase().as_str(), "next" | "random") {
                let set = text("set");
                let pool: Vec<&crate::model::Tile> = glb.tiles.iter().filter(|t| set.is_empty() || t.set.eq_ignore_ascii_case(&set)).collect();
                if pool.is_empty() {
                    return Err(format!("GLB {layer} has no tile in set \"{set}\""));
                }
                let now = pool.iter().position(|t| r.tiles.iter().any(|a| a.layer == layer && a.name == t.name));
                let i = if name.eq_ignore_ascii_case("next") {
                    now.map_or(0, |i| (i + 1) % pool.len())
                } else {
                    let roll = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize / 7).unwrap_or(0);
                    // Another one than the one showing, when there's a choice
                    match now {
                        Some(n) if pool.len() > 1 => (n + 1 + roll % (pool.len() - 1)) % pool.len(),
                        _ => roll % pool.len(),
                    }
                };
                name = pool[i].name.clone();
            }
            let mut tile = if !name.is_empty() {
                let t = glb.tiles.iter().find(|t| t.name.eq_ignore_ascii_case(&name)).ok_or_else(|| format!("GLB {layer} has no tile \"{name}\""))?;
                ActiveTile { layer, name: t.name.clone(), binds: t.binds.clone(), weight, grid: None, key: None }
            } else {
                let mat_name = text("material");
                let material = glb.materials.iter().position(|m| m.eq_ignore_ascii_case(&mat_name)).ok_or_else(|| format!("GLB {layer} has no material \"{mat_name}\""))?;
                let whole = |k: &str, d: u32| number(args, k, d as f32, 1.0, 1024.0).round() as u32;
                let (columns, rows) = (whole("columns", 1), whole("rows", 1));
                // The grid tile on that material now: "next" goes on from it
                let now = r.tiles.iter().find(|t| t.layer == layer && t.binds.iter().any(|(m, _)| *m == material)).and_then(|t| t.grid).map(|g| g.2);
                let cell = match text("cell").to_ascii_lowercase().as_str() {
                    "next" => now.map_or(0, |c| c + 1),
                    "random" => (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) / 7) % (columns * rows),
                    _ => number(args, "cell", 0.0, 0.0, 1_000_000.0).round() as u32,
                } % (columns * rows);
                ActiveTile { layer, name: String::new(), binds: vec![(material, grid_cell(columns, rows, cell))], weight, grid: Some((columns, rows, cell)), key: None }
            };
            // A shape key held with it
            let key = text("key");
            if !key.is_empty() {
                let found = glb.keys.iter().find(|k| k.eq_ignore_ascii_case(&key)).ok_or_else(|| format!("GLB {layer} has no shape key \"{key}\""))?;
                tile.key = Some((found.clone(), number(args, "key_value", 1.0, 0.0, 1.0)));
            }
            let mut materials: Vec<usize> = tile.binds.iter().map(|(m, _)| *m).collect();
            materials.sort();
            *undo = Some((format!("tile {layer} {materials:?}"), Undo::Tiles { layer, materials: materials.clone(), before: r.tiles_on(layer, &materials), fade }));
            // Already showing: the same named tile, or the same cell of a grid
            let showing = r.tiles.iter().any(|t| t.layer == layer && if tile.name.is_empty() { t.name.is_empty() && t.binds == tile.binds } else { t.name == tile.name });
            let on = match state.as_str() {
                "off" => false,
                "toggle" => !showing,
                _ => true,
            };
            // One tile per material: what was on those materials goes (the
            // crossfade its materials take, on and off)
            tile.weight = weight;
            r.put_tiles(layer, &materials, if on { vec![tile] } else { Vec::new() }, fade);
        }
        "color" => {
            let slot = args.get("slot").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))).ok_or("no slot")? as u32;
            let color = text("color");
            drop(r);
            let rgb = if color.eq_ignore_ascii_case("reset") { None } else { Some(parse_color(&color).ok_or_else(|| format!("colour \"{color}\": #rrggbb or reset"))?) };
            let before = crate::source::get_color_setting(source, slot).map_or_else(|| "reset".to_string(), |c| format!("#{:02x}{:02x}{:02x}", (c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8));
            *undo = Some((format!("color {slot}"), Undo::Args(json!({ "action": "color", "slot": slot, "color": before }))));
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
    if action == "tile_previews" {
        return match targets.first() {
            None => json!({ "ok": false, "error": "no Puppet source" }),
            Some((_, _, remote)) => match previews(&args, remote) {
                Ok(p) => json!({ "ok": true, "previews": p }),
                Err(e) => json!({ "ok": false, "error": e }),
            },
        }
        .to_string();
    }
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
                let mut groups: Vec<String> = (0..r.glbs.len()).map(|l| group_of(&r.groups, r.slot(l)).to_string()).filter(|g| !g.is_empty()).collect();
                // Each GLB's tile sets: {set: [names, in order]}
                let tile_sets = |g: &GlbInfo| {
                    let mut m = serde_json::Map::new();
                    for t in &g.tiles {
                        m.entry(t.set.clone()).or_insert_with(|| json!([])).as_array_mut().unwrap().push(json!(t.name));
                    }
                    Value::Object(m)
                };
                groups.sort();
                groups.dedup();
                json!({
                    "source": name,
                    "glbs": r.glbs.iter().enumerate().map(|(i, g)| json!({ "index": i, "name": g.name, "keys": g.keys, "visible": !r.is_hidden(i), "group": group_of(&r.groups, r.slot(i)),
                        "materials": g.materials, "tiles": g.tiles.iter().map(|t| t.name.clone()).collect::<Vec<_>>(), "tile_sets": tile_sets(g),
                        "tiles_on": r.tiles.iter().filter(|t| t.layer == i).map(|t| if t.name.is_empty() { t.grid.map(|(c, rr, cell)| format!("{}x{} cell {cell}", c, rr)).unwrap_or_default() } else { t.name.clone() }).collect::<Vec<_>>() })).collect::<Vec<_>>(),
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

/// `tile_previews`: each tile's picture (every material it moves, side by side), or each cell of a grid
fn previews(args: &Value, remote: &Shared) -> Result<Vec<Value>, String> {
    let (layer, glb) = {
        let r = remote.lock().unwrap();
        let layer = r.layer(args.get("glb"))?;
        (layer, r.glbs.get(layer).cloned().unwrap_or_default())
    };
    let model = glb.model.ok_or_else(|| format!("GLB {layer} isn't loaded"))?;
    let size = number(args, "size", 64.0, 8.0, 256.0) as u32;
    let entry = |name: String, material: usize, st: [f32; 4]| crate::thumbs::thumb(&model, material, st, size).map(|png| json!({ "name": name, "png": crate::thumbs::base64(&png) }));
    let material = args.get("material").and_then(|v| v.as_str()).unwrap_or("").trim();
    if material.is_empty() {
        Ok(model
            .tiles
            .iter()
            .filter_map(|t| crate::thumbs::thumbs(&model, &t.binds, size).map(|png| json!({ "name": t.name, "set": t.set, "png": crate::thumbs::base64(&png) })))
            .collect())
    } else {
        let m = model.materials.iter().position(|x| x.name.eq_ignore_ascii_case(material)).ok_or_else(|| format!("GLB {layer} has no material \"{material}\""))?;
        let whole = |k: &str| number(args, k, 1.0, 1.0, 64.0).round() as u32;
        let (columns, rows) = (whole("columns"), whole("rows"));
        Ok((0..columns * rows).filter_map(|cell| entry(cell.to_string(), m, grid_cell(columns, rows, cell))).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tile_has_a_picture() {
        // THEATER_PUPPET_TILED_GLB: HavocFFT.vrm through Theater Rig's Export
        let Ok(path) = std::env::var("THEATER_PUPPET_TILED_GLB") else { return };
        let model = Arc::new(crate::model::load(std::path::Path::new(&path)).unwrap());
        let remote: Shared = Arc::new(Mutex::new(Remote { glbs: vec![GlbInfo { model: Some(model.clone()), ..Default::default() }], ..Default::default() }));
        let named = previews(&json!({}), &remote).unwrap();
        assert_eq!(named.len(), model.tiles.len());
        let grid = previews(&json!({ "material": "Wolf3D_Eye", "columns": 2, "rows": 2, "size": 32 }), &remote).unwrap();
        assert_eq!(grid.len(), 4);
        // Real PNGs ("\x89PNG" in base64)
        assert!(named.iter().chain(&grid).all(|p| p["png"].as_str().unwrap().starts_with("iVBORw0KGgo")));
        // THEATER_PUPPET_THUMBS_OUT: a folder to save them in, to look at
        if let Ok(dir) = std::env::var("THEATER_PUPPET_THUMBS_OUT") {
            for (i, p) in named.iter().chain(&grid).enumerate() {
                let png = crate::thumbs::base64_decode(p["png"].as_str().unwrap());
                std::fs::write(std::path::Path::new(&dir).join(format!("{i}_{}.png", p["name"].as_str().unwrap())), png).unwrap();
            }
        }
    }

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
        let once = |shape| Drive::Once { from: 0.2, to: 0.8, duration: 2.0, shape, hide_at_end: false, fade: 0.0 };
        assert_eq!(at(once(Shape::Up), 0.0), Some(0.2));
        assert!((at(once(Shape::Up), 1.0).unwrap() - 0.5).abs() < 1e-6);
        assert!((at(once(Shape::Down), 0.5).unwrap() - 0.65).abs() < 1e-6);
        assert!((at(once(Shape::PingPong), 1.0).unwrap() - 0.8).abs() < 1e-6);
        assert!((at(once(Shape::PingPong), 2.0).unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(at(once(Shape::Up), 2.1), None);
    }

    #[test]
    fn glbs_are_found_by_number_or_name() {
        let r = Remote { glbs: vec![GlbInfo { name: "Armature".into(), ..Default::default() }, GlbInfo { name: "Hat".into(), ..Default::default() }], ..Default::default() };
        assert_eq!(r.layer(None), Ok(0));
        assert_eq!(r.layer(Some(&json!(1))), Ok(1));
        assert_eq!(r.layer(Some(&json!("hat"))), Ok(1));
        assert_eq!(r.layer(Some(&json!("1"))), Ok(1));
        assert!(r.layer(Some(&json!(2))).is_err());
        assert!(r.layer(Some(&json!("nope"))).is_err());
    }

    #[test]
    fn one_glb_of_a_group_shows() {
        let groups = vec!["".to_string(), "hats".to_string(), "hats".to_string(), "".to_string(), "hats".to_string()];
        // Three hats set to show: the first does
        assert_eq!(effective(&[true, true, true, true, true], &groups), [true, true, false, true, false]);
        // Showing the third hat hides the others
        assert_eq!(showing(&[true, true, false, true, false], &groups, 4, true), [true, false, false, true, true]);
        // Hiding one leaves the rest
        assert_eq!(showing(&[true, true, false, true, false], &groups, 1, false), [true, false, false, true, false]);
        // No group: nothing else changes
        assert_eq!(showing(&[true, true, false, false, false], &groups, 3, true), [true, true, false, true, false]);
        // A slot past the settings shows (a GLB just added)
        assert_eq!(showing(&[true], &groups, 2, true), [true, false, true]);
    }

    #[test]
    fn a_group_shows_a_name_the_next_one_or_none() {
        // The avatar, then hats in Also draw 1 and 3 (2 didn't load)
        let glb = |n: &str| GlbInfo { name: n.into(), ..Default::default() };
        let mut r = Remote {
            glbs: vec![glb("Armature"), glb("Cap"), glb("Crown")],
            layer_slot: vec![0, 1, 3],
            shown: vec![true, true, true, true],
            groups: vec!["".into(), "hats".into(), "".into(), "Hats".into()],
            ..Default::default()
        };
        r.update_hidden();
        // Both set to show: the first of the group does
        assert_eq!(r.hidden, vec![false, false, true]);
        assert_eq!(r.in_group("hats", Some(&json!("crown"))), Ok(vec![true, false, true, true]));
        assert_eq!(r.in_group("hats", Some(&json!("next"))), Ok(vec![true, false, true, true]));
        assert_eq!(r.in_group("hats", Some(&json!("none"))), Ok(vec![true, false, true, false]));
        assert!(r.in_group("hats", Some(&json!("Armature"))).is_err());
        assert!(r.in_group("shoes", None).is_err());
        // From the crown, next wraps to the cap
        r.shown = vec![true, false, true, true];
        assert_eq!(r.in_group("HATS", None), Ok(vec![true, true, true, false]));
    }

    #[test]
    fn tiles_of_any_grid() {
        // 4 x 2: cell 5 is the second row, second column
        assert_eq!(grid_cell(4, 2, 5), [0.25, 0.5, 0.25, 0.5]);
        assert_eq!(grid_cell(3, 3, 9), grid_cell(3, 3, 0));
        let base = crate::model::UV_IDENTITY;
        let t = |weight| ActiveTile { layer: 0, name: "x".into(), binds: vec![(2, [0.5, 0.5, 0.5, 0.0])], weight, grid: None, key: None };
        assert_eq!(tiled(base, &[t(1.0)], 0, 2), Some([0.5, 0.5, 0.5, 0.0]));
        assert_eq!(tiled(base, &[t(0.5)], 0, 2), Some([0.75, 0.75, 0.25, 0.0]));
        assert_eq!(tiled(base, &[t(1.0)], 0, 1), None);
        assert_eq!(tiled(base, &[t(1.0)], 1, 2), None);
    }

    #[test]
    fn a_glb_fades_over_its_seconds() {
        let mut r = Remote { layer_slot: vec![0, 1], shown: vec![true, true], ..Default::default() };
        r.update_hidden();
        assert_eq!(r.fade, vec![1.0, 1.0]);
        // Hidden with a 0.5 s fade: halfway after a quarter second
        r.pending_fade = 0.5;
        r.shown[1] = false;
        r.update_hidden();
        r.step_fade(0.25);
        assert!((r.fade[1] - 0.5).abs() < 1e-6 && r.fade[0] == 1.0);
        r.step_fade(1.0);
        assert_eq!(r.fade[1], 0.0);
        // Shown again with no fade: at once
        r.shown[1] = true;
        r.update_hidden();
        r.step_fade(0.01);
        assert_eq!(r.fade[1], 1.0);
    }

    #[test]
    fn a_tile_holds_its_key_and_its_timer_puts_back_the_one_before() {
        let tile = |n: &str| crate::model::Tile { name: n.into(), set: "eyes".into(), order: 0, binds: vec![(0, [0.5, 0.5, 0.0, 0.0])] };
        let remote: Shared = Arc::new(Mutex::new(Remote {
            glbs: vec![GlbInfo { model: None, name: "Armature".into(), keys: vec!["Blush".into(), "Fcl_MTH_A".into()], materials: vec!["Skin".into()], tiles: vec![tile("Red"), tile("Blue")] }],
            ..Default::default()
        }));
        let source = std::ptr::null_mut();
        let start = Instant::now();
        let at = |s: f32| start + Duration::from_secs_f32(s);
        // Red, for good
        apply_at(&json!({ "action": "tile", "name": "Red" }), source, &remote, at(0.0)).unwrap();
        // Blue with its key, 5 s; then again 3 s later for 5 s more
        let blue = json!({ "action": "tile", "name": "blue", "key": "blush", "key_value": 0.6, "timer": 5 });
        apply_at(&blue, source, &remote, at(1.0)).unwrap();
        assert!(apply_at(&json!({ "action": "tile", "name": "Blue", "key": "nope" }), source, &remote, at(1.0)).is_err());
        {
            let mut r = remote.lock().unwrap();
            assert_eq!(r.tiles.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Blue"]);
            assert_eq!(std::mem::take(&mut r.shapes), vec![(0, "Blush".to_string(), Some(Drive::Hold(0.6)))]);
        }
        apply_at(&blue, source, &remote, at(4.0)).unwrap();
        assert_eq!(remote.lock().unwrap().timers.len(), 1);
        remote.lock().unwrap().shapes.clear();
        // The first timer's time: nothing yet
        run_due_at(source, &remote, at(6.5));
        assert_eq!(remote.lock().unwrap().tiles[0].name, "Blue");
        // The second's: Red back (as before the first), the key let go
        run_due_at(source, &remote, at(9.5));
        let r = remote.lock().unwrap();
        assert_eq!(r.tiles.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Red"]);
        assert_eq!(r.shapes, vec![(0, "Blush".to_string(), None)]);
        assert!(r.timers.is_empty());
    }

    #[test]
    fn next_and_random_go_through_a_set() {
        let tile = |n: &str, set: &str, order: i64, m: usize| crate::model::Tile { name: n.into(), set: set.into(), order, binds: vec![(m, [0.5, 0.5, order as f32 * 0.5, 0.0])] };
        let remote: Shared = Arc::new(Mutex::new(Remote {
            glbs: vec![GlbInfo { materials: vec!["Eye".into(), "Mouth".into()], tiles: vec![tile("Open", "eyes", 0, 0), tile("Shut", "eyes", 1, 0), tile("Smile", "mouths", 0, 1)], ..Default::default() }],
            ..Default::default()
        }));
        let on = |r: &Shared| r.lock().unwrap().tiles.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        let go = |args: Value| apply_at(&args, std::ptr::null_mut(), &remote, Instant::now()).unwrap();
        go(json!({ "action": "tile", "name": "next", "set": "eyes" }));
        assert_eq!(on(&remote), ["Open"]);
        go(json!({ "action": "tile", "name": "next", "set": "Eyes" }));
        assert_eq!(on(&remote), ["Shut"]);
        go(json!({ "action": "tile", "name": "next", "set": "eyes" }));
        assert_eq!(on(&remote), ["Open"]);
        // Random, with a choice: never the one showing
        for _ in 0..5 {
            go(json!({ "action": "tile", "name": "random", "set": "eyes" }));
            assert_eq!(on(&remote), ["Shut"]);
            go(json!({ "action": "tile", "name": "Open" }));
        }
        // Another set: its own material, both show
        go(json!({ "action": "tile", "name": "next", "set": "mouths" }));
        assert_eq!(on(&remote), ["Open", "Smile"]);
        assert!(apply_at(&json!({ "action": "tile", "name": "next", "set": "hats" }), std::ptr::null_mut(), &remote, Instant::now()).is_err());
    }

    #[test]
    fn a_timed_shape_key_goes_off() {
        let remote: Shared = Arc::new(Mutex::new(Remote { glbs: vec![GlbInfo { name: "A".into(), keys: vec!["Fcl_MTH_A".into()], ..Default::default() }], ..Default::default() }));
        let start = Instant::now();
        apply_at(&json!({ "action": "shape", "key": "Fcl_MTH_A", "timer": "2" }), std::ptr::null_mut(), &remote, start).unwrap();
        remote.lock().unwrap().shapes.clear();
        run_due_at(std::ptr::null_mut(), &remote, start + Duration::from_secs(3));
        assert_eq!(remote.lock().unwrap().shapes, vec![(0, "Fcl_MTH_A".to_string(), None)]);
    }

    #[test]
    fn colours_are_read() {
        assert_eq!(parse_color("#ff8000"), Some([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(parse_color("00FF00"), Some([0.0, 1.0, 0.0]));
        assert_eq!(parse_color("red"), None);
    }
}
