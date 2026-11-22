//! The colour slots as they show, frame by frame: each slot's colour (the
//! source's setting, else the slot's own), a change gliding from what showed
//! to the new one over the command's fade (red, green and blue moving, as a
//! tile crossfades), a gradient (one colour at the top, another at the
//! bottom, or side to side, front to back: on the avatar at rest), and the
//! rainbow: every slot its own hue around the wheel, still or turning, or
//! flowing along the part; each eased in and out over its fade.
//!
//! A slot showing its own colour, nothing on it, isn't in what's drawn: its
//! materials keep their own (two materials of one slot can differ).

use std::collections::HashMap;

/// Along the avatar at rest: 0 side to side (its left first), 1 top to
/// bottom, 2 front to back
pub type Axis = usize;

pub fn axis_from(text: &str) -> Axis {
    match text.trim() {
        "side" | "left" | "x" => 0,
        "front" | "z" => 2,
        _ => 1,
    }
}

/// The rainbow on a Puppet's slots
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rainbow {
    /// Turns of the wheel a second (0: still)
    pub speed: f32,
    /// Seconds it takes to come in and to go
    pub fade: f32,
    /// Flowing along the part (Some: the direction), else one hue a slot
    pub flow: Option<Axis>,
}

/// A gradient: `top` at the start of the direction, `bottom` at its end
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub top: [f32; 3],
    pub bottom: [f32; 3],
    pub axis: Axis,
    /// The slots it's on (None: every one)
    pub slots: Option<Vec<u32>>,
    pub fade: f32,
}

/// What a slot shows: `a` at the start of `axis`, `b` at its end (the same
/// for one colour), and a rainbow flowing along it: [its hue at the start,
/// how much of the wheel along the part, how much shows]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub axis: Axis,
    pub flow: [f32; 3],
}

impl Look {
    pub fn solid(c: [f32; 3]) -> Look {
        Look { a: c, b: c, axis: 1, flow: [0.0; 3] }
    }
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
    /// What each slot showed last frame (its solid colour)
    shown: HashMap<u32, [f32; 3]>,
    /// Each slot's colour as last seen (None: its own)
    target: HashMap<u32, Option<[f32; 3]>>,
    glides: HashMap<u32, Glide>,
    rainbow: Option<Rainbow>,
    /// How much the rainbow shows (0–1), easing toward on or off
    rainbow_weight: f32,
    rainbow_fade: f32,
    /// The rainbow's last way (kept while it fades out)
    rainbow_last: Option<Rainbow>,
    gradient: Option<Gradient>,
    gradient_weight: f32,
    gradient_last: Option<Gradient>,
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

/// `w` eased toward `goal` over `seconds`
fn toward(w: f32, goal: f32, dt: f32, seconds: f32) -> f32 {
    let rate = if seconds > 0.0 { dt / seconds } else { 1.0 };
    if w < goal { (w + rate).min(goal) } else { (w - rate).max(goal) }
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
            self.rainbow_last = Some(r);
        } else if let Some(f) = fade {
            self.rainbow_fade = f;
        }
        self.rainbow = rainbow;
    }

    pub fn rainbow(&self) -> Option<Rainbow> {
        self.rainbow
    }

    /// The gradient on (Some) or off (None, over `fade` when given)
    pub fn set_gradient(&mut self, gradient: Option<Gradient>, fade: Option<f32>) {
        match gradient {
            Some(g) => {
                if self.gradient_weight > 0.0 && self.gradient.is_none() {
                    // Back on while it was going: from where it is
                }
                self.gradient_last = Some(g.clone());
                self.gradient = Some(g);
            }
            None => {
                if let (Some(f), Some(last)) = (fade, self.gradient_last.as_mut()) {
                    last.fade = f;
                }
                self.gradient = None;
            }
        }
    }

    /// `dt` seconds on: what each slot shows. `set`: the slots given a
    /// colour (the source's settings); `slots`: every slot there is with its
    /// own colour. Only slots not at their own colour are in what it returns.
    pub fn step(&mut self, dt: f32, set: &HashMap<u32, [f32; 3]>, slots: &[(u32, [f32; 3])]) -> HashMap<u32, Look> {
        self.time += dt;
        self.next_fade_wait -= dt;
        if self.next_fade_wait <= 0.0 {
            self.next_fade = 0.0;
        }
        // The rainbow and the gradient easing in or out
        self.rainbow_weight = toward(self.rainbow_weight, if self.rainbow.is_some() { 1.0 } else { 0.0 }, dt, self.rainbow_fade);
        let gfade = self.gradient_last.as_ref().map_or(0.0, |g| g.fade);
        self.gradient_weight = toward(self.gradient_weight, if self.gradient.is_some() { 1.0 } else { 0.0 }, dt, gfade);
        let rainbow = self.rainbow.or(self.rainbow_last);
        let speed = rainbow.map_or(0.0, |r| r.speed);
        let gradient = self.gradient.clone().or_else(|| self.gradient_last.clone());

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
            self.shown.insert(slot, c);
            let mut look = Look::solid(c);
            let mut touched = want.is_some() || gliding;
            // The gradient: top and bottom from the slot's colour, eased
            if let Some(g) = gradient.as_ref().filter(|g| self.gradient_weight > 0.0 && g.slots.as_ref().is_none_or(|s| s.contains(&slot))) {
                let w = ease(self.gradient_weight);
                look.a = lerp(c, g.top, w);
                look.b = lerp(c, g.bottom, w);
                look.axis = g.axis;
                touched = true;
            }
            if self.rainbow_weight > 0.0 {
                let w = ease(self.rainbow_weight);
                let h = i as f32 / count + self.time * speed;
                match rainbow.and_then(|r| r.flow) {
                    // Flowing along the part: the shader turns the hue along it
                    Some(axis) => {
                        look.axis = axis;
                        look.flow = [h, 1.0, w];
                    }
                    // Each slot its own hue, the wheel turning
                    None => {
                        look.a = lerp(look.a, hue(h), w);
                        look.b = lerp(look.b, hue(h), w);
                    }
                }
                touched = true;
            }
            if touched {
                out.insert(slot, look);
            }
        }
        if changed {
            self.next_fade = 0.0;
        }
        if self.gradient.is_none() && self.gradient_weight <= 0.0 {
            self.gradient_last = None;
        }
        if self.rainbow.is_none() && self.rainbow_weight <= 0.0 {
            self.rainbow_last = None;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f32; 3] = [1.0, 0.0, 0.0];
    const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
    const PINK: [f32; 3] = [1.0, 0.4, 0.7];

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
        assert!(close(out[&2].a, RED) && close(out[&2].b, RED), "no fade: at once, one colour");
    }

    #[test]
    fn a_change_glides_over_its_fade() {
        let mut fx = ColorFx::default();
        let slots = [(3, BLUE)];
        fx.step(0.1, &HashMap::new(), &slots);
        fx.fade_next(2.0);
        let set = HashMap::from([(3, RED)]);
        let start = fx.step(0.0, &set, &slots);
        assert!(close(start[&3].a, BLUE), "it starts from what showed");
        let half = fx.step(1.0, &set, &slots);
        assert!(close(half[&3].a, [0.5, 0.0, 0.5]), "half way: red and blue each half ({:?})", half[&3]);
        let end = fx.step(1.5, &set, &slots);
        assert!(close(end[&3].a, RED));
        let back = fx.step(0.1, &HashMap::new(), &slots);
        assert!(back.is_empty());
    }

    #[test]
    fn the_rainbow_gives_each_slot_its_hue_and_turns_then_goes() {
        let mut fx = ColorFx::default();
        let slots = [(1, RED), (2, RED), (3, RED)];
        fx.set_rainbow(Some(Rainbow { speed: 0.0, fade: 0.0, flow: None }), None);
        let out = fx.step(0.1, &HashMap::new(), &slots);
        assert_eq!(out.len(), 3);
        assert!(!close(out[&1].a, out[&2].a) && !close(out[&2].a, out[&3].a) && !close(out[&1].a, out[&3].a), "{out:?}");
        fx.set_rainbow(Some(Rainbow { speed: 0.5, fade: 0.0, flow: None }), None);
        let a = fx.step(0.1, &HashMap::new(), &slots)[&1].a;
        let b = fx.step(0.5, &HashMap::new(), &slots)[&1].a;
        assert!(!close(a, b));
        fx.set_rainbow(None, Some(1.0));
        let mid = fx.step(0.5, &HashMap::new(), &slots);
        assert_eq!(mid.len(), 3);
        let gone = fx.step(0.6, &HashMap::new(), &slots);
        assert!(gone.is_empty(), "{gone:?}");
    }

    #[test]
    fn a_gradient_goes_from_top_to_bottom_on_its_slots_and_eases() {
        let mut fx = ColorFx::default();
        let slots = [(2, RED), (5, RED)];
        fx.set_gradient(Some(Gradient { top: PINK, bottom: BLUE, axis: 1, slots: Some(vec![2]), fade: 1.0 }), None);
        let half = fx.step(0.5, &HashMap::new(), &slots);
        assert!(!half.contains_key(&5), "only its slots");
        assert!(!close(half[&2].a, RED) && !close(half[&2].a, PINK), "half way in: {:?}", half[&2]);
        let full = fx.step(0.6, &HashMap::new(), &slots);
        assert!(close(full[&2].a, PINK) && close(full[&2].b, BLUE) && full[&2].axis == 1);
        // Off over 1 s: gone after
        fx.set_gradient(None, Some(1.0));
        fx.step(0.5, &HashMap::new(), &slots);
        let gone = fx.step(0.6, &HashMap::new(), &slots);
        assert!(gone.is_empty(), "{gone:?}");
    }

    #[test]
    fn a_flowing_rainbow_runs_along_the_part() {
        let mut fx = ColorFx::default();
        fx.set_rainbow(Some(Rainbow { speed: 0.5, fade: 0.0, flow: Some(1) }), None);
        let out = fx.step(0.1, &HashMap::new(), &[(1, RED)]);
        let l = out[&1];
        assert_eq!((l.axis, l.flow[1], l.flow[2]), (1, 1.0, 1.0), "{l:?}");
        let later = fx.step(0.5, &HashMap::new(), &[(1, RED)])[&1];
        assert!((later.flow[0] - l.flow[0] - 0.25).abs() < 1e-4, "it moves along");
    }

    #[test]
    fn hues_go_round() {
        assert!(close(hue(0.0), [1.0, 0.15, 0.15]));
        assert!(close(hue(1.0), hue(0.0)));
        assert!(close(hue(-0.25), hue(0.75)));
        assert_eq!((axis_from("down"), axis_from("side"), axis_from("front")), (1, 0, 2));
    }
}
