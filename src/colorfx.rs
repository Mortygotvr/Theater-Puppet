//! The colour slots as they show, frame by frame: each slot's colour (the
//! source's setting, else the slot's own), a change gliding from what showed
//! to the new one over the command's fade (red, green and blue moving, as a
//! tile crossfades), and the rainbow: every slot its own hue around the
//! wheel, still or turning, eased in and out over its fade.
//!
//! A slot showing its own colour, nothing on it, isn't in what's drawn: its
//! materials keep their own (two materials of one slot can differ).

use std::collections::HashMap;

/// The rainbow on a Puppet's slots
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rainbow {
    /// Turns of the wheel a second (0: still)
    pub speed: f32,
    /// Seconds it takes to come in and to go
    pub fade: f32,
}

#[derive(Clone, Copy, Debug)]
struct Glide {
    from: [f32; 3],
    to: [f32; 3],
    time: f32,
    seconds: f32,
}

#[derive(Default)]
pub struct ColorFx {
    /// What each slot showed last frame
    shown: HashMap<u32, [f32; 3]>,
    /// Each slot's colour as last seen (None: its own)
    target: HashMap<u32, Option<[f32; 3]>>,
    glides: HashMap<u32, Glide>,
    rainbow: Option<Rainbow>,
    /// How much the rainbow shows (0–1), easing toward on or off
    rainbow_weight: f32,
    rainbow_fade: f32,
    time: f32,
    /// The fade the next change takes (the last colour command's), and how
    /// long it waits for the change to come in (its settings run after it)
    next_fade: f32,
    next_fade_wait: f32,
}

/// A hue (turns, any number) at full colour, a little lifted (never black)
pub fn hue(h: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    [r, g, b].map(|c: f32| 0.15 + c * 0.85)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Eased: slow out of the old colour, slow into the new
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl ColorFx {
    /// The next colour change glides over `seconds`
    pub fn fade_next(&mut self, seconds: f32) {
        self.next_fade = seconds.max(0.0);
        self.next_fade_wait = 2.0;
    }

    /// The rainbow on (Some) or off (None, going over the fade it came with,
    /// or `fade` when given)
    pub fn set_rainbow(&mut self, rainbow: Option<Rainbow>, fade: Option<f32>) {
        if let Some(r) = rainbow {
            self.rainbow_fade = r.fade;
            if self.rainbow.is_none() && self.rainbow_weight <= 0.0 {
                // From the start of the wheel each time it comes in
                self.time = 0.0;
            }
        } else if let Some(f) = fade {
            self.rainbow_fade = f;
        }
        self.rainbow = rainbow;
    }

    pub fn rainbow(&self) -> Option<Rainbow> {
        self.rainbow
    }

    /// `dt` seconds on: what each slot shows. `set`: the slots given a
    /// colour (the source's settings); `slots`: every slot there is with its
    /// own colour. Only slots not at their own colour are in what it returns.
    pub fn step(&mut self, dt: f32, set: &HashMap<u32, [f32; 3]>, slots: &[(u32, [f32; 3])]) -> HashMap<u32, [f32; 3]> {
        self.time += dt;
        self.next_fade_wait -= dt;
        if self.next_fade_wait <= 0.0 {
            self.next_fade = 0.0;
        }
        // The rainbow easing in or out
        let goal = if self.rainbow.is_some() { 1.0 } else { 0.0 };
        let rate = if self.rainbow_fade > 0.0 { dt / self.rainbow_fade } else { 1.0 };
        self.rainbow_weight = if self.rainbow_weight < goal { (self.rainbow_weight + rate).min(goal) } else { (self.rainbow_weight - rate).max(goal) };
        let speed = self.rainbow.map_or(0.0, |r| r.speed);

        let mut changed = false;
        let mut out = HashMap::new();
        let count = slots.len().max(1) as f32;
        for (i, &(slot, own)) in slots.iter().enumerate() {
            let want = set.get(&slot).copied();
            let to = want.unwrap_or(own);
            // A new colour: from what shows now, over the command's fade
            let before = self.target.insert(slot, want);
            if before.is_some_and(|b| b != want) {
                changed = true;
                let from = self.shown.get(&slot).copied().unwrap_or(own);
                if self.next_fade > 0.0 {
                    self.glides.insert(slot, Glide { from, to, time: 0.0, seconds: self.next_fade });
                } else {
                    self.glides.remove(&slot);
                }
            }
            let mut c = to;
            let mut gliding = false;
            if let Some(g) = self.glides.get_mut(&slot) {
                g.time += dt;
                g.to = to;
                c = lerp(g.from, g.to, ease(g.time / g.seconds));
                gliding = g.time < g.seconds;
            }
            if !gliding {
                self.glides.remove(&slot);
            }
            if self.rainbow_weight > 0.0 {
                // Each slot its own hue, the wheel turning
                c = lerp(c, hue(i as f32 / count + self.time * speed), ease(self.rainbow_weight));
            }
            self.shown.insert(slot, c);
            if want.is_some() || gliding || self.rainbow_weight > 0.0 {
                out.insert(slot, c);
            }
        }
        if changed {
            self.next_fade = 0.0;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f32; 3] = [1.0, 0.0, 0.0];
    const BLUE: [f32; 3] = [0.0, 0.0, 1.0];

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3)
    }

    #[test]
    fn a_slot_at_its_own_colour_is_left_alone() {
        let mut fx = ColorFx::default();
        let out = fx.step(0.1, &HashMap::new(), &[(1, RED), (2, BLUE)]);
        assert!(out.is_empty());
        let out = fx.step(0.1, &HashMap::from([(2, RED)]), &[(1, RED), (2, BLUE)]);
        assert_eq!(out.len(), 1);
        assert!(close(out[&2], RED), "no fade: at once");
    }

    #[test]
    fn a_change_glides_over_its_fade() {
        let mut fx = ColorFx::default();
        let slots = [(3, BLUE)];
        fx.step(0.1, &HashMap::new(), &slots);
        fx.fade_next(2.0);
        let set = HashMap::from([(3, RED)]);
        let start = fx.step(0.0, &set, &slots);
        assert!(close(start[&3], BLUE), "it starts from what showed");
        let half = fx.step(1.0, &set, &slots);
        assert!(close(half[&3], [0.5, 0.0, 0.5]), "half way: red and blue each half ({:?})", half[&3]);
        let end = fx.step(1.5, &set, &slots);
        assert!(close(end[&3], RED));
        // Back to its own, the next fade not given: at once
        let back = fx.step(0.1, &HashMap::new(), &slots);
        assert!(back.is_empty());
    }

    #[test]
    fn the_rainbow_gives_each_slot_its_hue_and_turns_then_goes() {
        let mut fx = ColorFx::default();
        let slots = [(1, RED), (2, RED), (3, RED)];
        fx.set_rainbow(Some(Rainbow { speed: 0.0, fade: 0.0 }), None);
        let out = fx.step(0.1, &HashMap::new(), &slots);
        assert_eq!(out.len(), 3);
        assert!(!close(out[&1], out[&2]) && !close(out[&2], out[&3]) && !close(out[&1], out[&3]), "{out:?}");
        // Turning: the hues move
        fx.set_rainbow(Some(Rainbow { speed: 0.5, fade: 0.0 }), None);
        let a = fx.step(0.1, &HashMap::new(), &slots)[&1];
        let b = fx.step(0.5, &HashMap::new(), &slots)[&1];
        assert!(!close(a, b));
        // Off over 1 s: half way at 0.5 s, gone after
        fx.set_rainbow(None, Some(1.0));
        let mid = fx.step(0.5, &HashMap::new(), &slots);
        assert_eq!(mid.len(), 3);
        let gone = fx.step(0.6, &HashMap::new(), &slots);
        assert!(gone.is_empty(), "{gone:?}");
    }

    #[test]
    fn hues_go_round() {
        assert!(close(hue(0.0), [1.0, 0.15, 0.15]));
        assert!(close(hue(1.0), hue(0.0)));
        assert!(close(hue(-0.25), hue(0.75)));
    }
}
