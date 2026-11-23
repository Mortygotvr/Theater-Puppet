//! Water: a liquid filling a container (Theater Rig's Make Water). Its mesh is
//! the container's inside, closed (the inner wall and a lid). Everything above
//! its level is laid flat onto it, so at any level its sides fit the
//! container's walls and its top is the surface; the surface moves with waves
//! (several wave trains, up and down only: it never goes through the glass),
//! its light following them.
//!
//! The level: the shape key `water_empty` (0 full, the default; 1 empty), so
//! Blender's animation, Export Prop, a prefab's clip and the shape command
//! all fill and empty it. Its own deltas are not used. The waves: the
//! material's `theater_water` extras {height, size, speed} (metres, metres,
//! waves a second).
//!
//! Its up is the mesh's own +Y (glTF's up: Blender's Z), the rotation applied
//! (Make Water does).

use std::sync::OnceLock;
use std::time::Instant;

use glam::Vec3;

/// The shape key that empties it (0 full, 1 empty)
pub const EMPTY_KEY: &str = "water_empty";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Water {
    /// How high its waves (metres, crest over the level)
    pub height: f32,
    /// How long its longest waves (metres)
    pub size: f32,
    /// How many waves a second
    pub speed: f32,
}

impl Water {
    /// From a material's extras: `theater_water` {height, size, speed}
    pub fn from_extras(extras: &serde_json::Value) -> Option<Water> {
        let w = extras.get("theater_water")?;
        let f = |k: &str, d: f32| w.get(k).and_then(|v| v.as_f64()).map_or(d, |v| v as f32);
        Some(Water { height: f("height", 0.004).max(0.0), size: f("size", 0.06).max(1e-4), speed: f("speed", 0.8) })
    }
}

/// Seconds since the first water was drawn (the waves' clock)
pub fn now() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// The wave trains: direction, length and height (of the longest's), speed
const TRAINS: [([f32; 2], f32, f32, f32); 3] = [([1.0, 0.0], 1.0, 1.0, 1.0), ([0.6, 0.8], 0.62, 0.5, 1.3), ([-0.7, 0.7], 0.38, 0.3, 1.7)];

/// The surface's height at (x, z) and its slope (d/dx, d/dz)
fn waves(w: &Water, x: f32, z: f32, t: f32) -> (f32, f32, f32) {
    let (mut h, mut dx, mut dz) = (0.0, 0.0, 0.0);
    for (d, len, amp, spd) in TRAINS {
        let k = std::f32::consts::TAU / (w.size * len);
        let a = w.height * amp;
        let phase = k * (d[0] * x + d[1] * z) - std::f32::consts::TAU * w.speed * spd * t;
        h += a * phase.sin();
        let c = a * k * phase.cos();
        dx += c * d[0];
        dz += c * d[1];
    }
    (h, dx, dz)
}

/// The liquid at `empty` (0 full, 1 empty) and `t` seconds: its vertices
/// (its own, shape keys added) with those above the level laid onto it, the
/// waves on them; their normals. Empty: all of them in one point (nothing shows)
pub fn surface(w: &Water, positions: &[Vec3], normals: &[Vec3], empty: f32, t: f32) -> (Vec<Vec3>, Vec<Vec3>) {
    let (lo, hi) = positions.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    if positions.is_empty() || empty >= 0.999 {
        let at = positions.first().copied().unwrap_or(Vec3::ZERO);
        return (vec![at; positions.len()], vec![Vec3::Y; positions.len()]);
    }
    let level = hi - (hi - lo) * empty.clamp(0.0, 1.0);
    let mut p_out = Vec::with_capacity(positions.len());
    let mut n_out = Vec::with_capacity(positions.len());
    for (p, n) in positions.iter().zip(normals) {
        if p.y >= level - 1e-6 {
            let (h, dx, dz) = waves(w, p.x, p.z, t);
            p_out.push(Vec3::new(p.x, level + h, p.z));
            n_out.push(Vec3::new(-dx, 1.0, -dz).normalize());
        } else {
            p_out.push(*p);
            n_out.push(*n);
        }
    }
    (p_out, n_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A jar's inside: a ring at each height (narrower at its neck) and a lid
    fn jar() -> (Vec<Vec3>, Vec<Vec3>) {
        let mut p = Vec::new();
        for (y, r) in [(0.0, 0.04), (0.05, 0.045), (0.10, 0.045), (0.12, 0.03)] {
            for i in 0..12 {
                let a = i as f32 / 12.0 * std::f32::consts::TAU;
                p.push(Vec3::new(r * a.cos(), y, r * a.sin()));
            }
        }
        p.push(Vec3::new(0.0, 0.12, 0.0)); // the lid's middle
        let n = vec![Vec3::X; p.len()];
        (p, n)
    }

    #[test]
    fn half_full_lays_the_top_onto_the_level_and_keeps_the_walls_below() {
        let w = Water { height: 0.0, size: 0.06, speed: 1.0 };
        let (p, n) = jar();
        let (q, m) = surface(&w, &p, &n, 0.5, 0.0);
        let level = 0.06;
        for (a, b) in p.iter().zip(&q) {
            if a.y > level {
                assert!((b.y - level).abs() < 1e-6, "{a} not laid onto the level: {b}");
                assert_eq!((a.x, a.z), (b.x, b.z), "moved sideways");
            } else {
                assert_eq!(a, b, "below the level, moved");
            }
        }
        // The flat top faces up; the walls below keep theirs
        assert!(m.iter().zip(&p).all(|(n, a)| if a.y > level { (*n - Vec3::Y).length() < 1e-6 } else { *n == Vec3::X }));
    }

    #[test]
    fn full_by_default_waves_up_and_down_only_and_empty_shows_nothing() {
        let w = Water { height: 0.004, size: 0.06, speed: 1.0 };
        let (p, n) = jar();
        let (q, _) = surface(&w, &p, &n, 0.0, 0.3);
        let top: Vec<_> = p.iter().zip(&q).filter(|(a, _)| a.y >= 0.12 - 1e-6).collect();
        assert!(top.iter().any(|(_, b)| (b.y - 0.12).abs() > 1e-4), "no waves");
        assert!(top.iter().all(|(a, b)| a.x == b.x && a.z == b.z && (b.y - 0.12).abs() <= 0.004 * 1.8 + 1e-6), "waves sideways or too high");
        // The waves move
        let (r, _) = surface(&w, &p, &n, 0.0, 0.8);
        assert!(q.iter().zip(&r).any(|(a, b)| (a.y - b.y).abs() > 1e-4));
        let (e, _) = surface(&w, &p, &n, 1.0, 0.0);
        assert!(e.iter().all(|v| *v == e[0]), "empty still shows");
    }

    /// THEATER_PUPPET_WATER_GLB=<Theater Rig's Make Water, exported as a prop>
    /// (its tests/run_water.py): its material has the waves, and half empty
    /// its top comes down to the middle of its height
    #[test]
    fn a_rig_water_glb_fills_and_waves() {
        let Ok(path) = std::env::var("THEATER_PUPPET_WATER_GLB") else { return };
        let model = crate::model::load(std::path::Path::new(&path)).unwrap();
        assert!(model.materials.iter().any(|m| m.water.is_some()), "no water material");
        let key = model.morph(EMPTY_KEY).expect("no water_empty key");
        let pose = crate::pose::Pose::rest(&model);
        let mut points: Vec<Vec<[f32; 4]>> = model.primitives.iter().map(|p| vec![[0.0; 4]; p.positions.len()]).collect();
        let mut normals = points.clone();
        let top = |pts: &Vec<Vec<[f32; 4]>>| pts.iter().flatten().map(|p| p[1]).fold(f32::MIN, f32::max);
        let low = |pts: &Vec<Vec<[f32; 4]>>| pts.iter().flatten().map(|p| p[1]).fold(f32::MAX, f32::min);
        let mut morph = vec![0.0; model.morph_names.len()];
        crate::pose::skin(&model, &pose, &morph, &mut points, &mut normals);
        let (full_top, bottom) = (top(&points), low(&points));
        morph[key] = 0.5;
        crate::pose::skin(&model, &pose, &morph, &mut points, &mut normals);
        let half = top(&points);
        println!("full {full_top:.4}, half {half:.4}, bottom {bottom:.4}");
        let middle = (full_top + bottom) / 2.0;
        assert!((half - middle).abs() < 0.006, "half empty: its top at {half}, not near {middle}");
    }
}
