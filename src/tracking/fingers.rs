//! Fingers from joint positions (the Leap's, MediaPipe's): how far each
//! joint is bent, and how far each finger is spread. Any space, any unit:
//! only directions count.

use glam::Vec3;

use super::Hand;

/// How far the fingers of a relaxed open hand fan out from the middle one
/// (toward the little finger: +), which the avatar's rest pose already has
const REST_SPREAD: [f32; 5] = [0.0, -0.10, 0.0, 0.09, 0.22];

fn angle(a: Vec3, b: Vec3) -> f32 {
    let (a, b) = (a.normalize_or_zero(), b.normalize_or_zero());
    if a == Vec3::ZERO || b == Vec3::ZERO { 0.0 } else { a.angle_between(b) }
}

/// One hand's fingers.
/// - `joints[f]`: the finger's four joints from its base to its tip (index
///   to little: knuckle, two joints, tip; thumb: its base at the wrist, two
///   joints, tip);
/// - `direction`: from the wrist to the knuckles; `palm`: out of the palm.
pub fn from_joints(left: bool, joints: &[[Vec3; 4]; 5], direction: Vec3, palm: Vec3) -> Hand {
    let dir = direction.normalize_or(Vec3::X);
    let palm = (palm - dir * palm.dot(dir)).normalize_or(Vec3::NEG_Y);
    // Across the palm, toward the little finger
    let little = dir.cross(palm) * if left { 1.0 } else { -1.0 };
    let mut hand = Hand::default();
    for f in 0..5 {
        let j = &joints[f];
        let (a, b, c) = (j[1] - j[0], j[2] - j[1], j[3] - j[2]);
        if a.length_squared() < 1e-12 {
            continue;
        }
        let an = a.normalize();
        if f == 0 {
            // The thumb's base: out to the side when open (away from the
            // little finger), folding across the palm as it closes
            hand.bend[0][0] = ((an.dot(little) + 0.6) * 0.9).clamp(0.0, 0.8);
            hand.bend[0][1] = angle(a, b).min(1.4);
            hand.bend[0][2] = angle(b, c).min(1.4);
        } else {
            // The knuckle: how far the first bone dips into the palm's side
            hand.bend[f][0] = an.dot(palm).clamp(-1.0, 1.0).asin().clamp(-0.4, 1.6);
            hand.bend[f][1] = angle(a, b).min(1.9);
            hand.bend[f][2] = angle(b, c).min(1.6);
            // Spread reads badly on a curled finger (its bone points into the palm)
            let flat = an - palm * an.dot(palm);
            if flat.length() > 0.5 {
                hand.spread[f] = ((flat.dot(little).atan2(flat.dot(dir)) - REST_SPREAD[f]) * 0.8).clamp(-0.35, 0.35);
            }
        }
    }
    hand
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A left hand as the avatar's is at rest: fingers along +X, palm down,
    /// the little finger at the back (-Z)
    fn open() -> [[Vec3; 4]; 5] {
        let finger = |z: f32| std::array::from_fn(|i| Vec3::new(0.09 + 0.03 * i as f32, 0.0, z));
        let thumb = std::array::from_fn(|i| Vec3::new(0.02 + 0.025 * i as f32, 0.0, 0.02 + 0.025 * i as f32));
        [thumb, finger(0.03), finger(0.01), finger(-0.01), finger(-0.03)]
    }

    #[test]
    fn an_open_hand_is_straight() {
        let hand = from_joints(true, &open(), Vec3::X, Vec3::NEG_Y);
        for f in 1..5 {
            assert!(hand.bend[f].iter().all(|b| b.abs() < 1e-3), "{:?}", hand.bend[f]);
        }
        assert!(hand.bend[0][0] < 0.05, "thumb base {}", hand.bend[0][0]);
    }

    #[test]
    fn a_curled_finger_bends_at_each_joint() {
        let mut joints = open();
        // The index: down at the knuckle by 60 degrees, then 60 more at each joint
        let step = |a: f32| Vec3::new(a.cos(), -a.sin(), 0.0) * 0.03;
        let base = joints[1][0];
        let (d1, d2, d3) = (step(60f32.to_radians()), step(120f32.to_radians()), step(180f32.to_radians()));
        joints[1] = [base, base + d1, base + d1 + d2, base + d1 + d2 + d3];
        let hand = from_joints(true, &joints, Vec3::X, Vec3::NEG_Y);
        let want = 60f32.to_radians();
        assert!(hand.bend[1].iter().all(|b| (b - want).abs() < 0.01), "{:?}", hand.bend[1]);
        // The others stay straight
        assert!(hand.bend[2].iter().all(|b| b.abs() < 1e-3));
    }

    #[test]
    fn spread_is_toward_the_little_finger_on_both_hands() {
        // The little finger fanned out toward the back
        let mut joints = open();
        let base = joints[4][0];
        let out = Vec3::new(0.8, 0.0, -0.6);
        joints[4] = std::array::from_fn(|i| base + out * 0.03 * i as f32);
        let left = from_joints(true, &joints, Vec3::X, Vec3::NEG_Y);
        assert!(left.spread[4] > 0.2, "{}", left.spread[4]);
        // The right hand is the left's mirror: the same fingers, the same numbers
        let mirrored = joints.map(|f| f.map(|p| Vec3::new(-p.x, p.y, p.z)));
        let right = from_joints(false, &mirrored, Vec3::NEG_X, Vec3::NEG_Y);
        assert!((right.spread[4] - left.spread[4]).abs() < 1e-4);
        assert!((right.bend[0][0] - left.bend[0][0]).abs() < 1e-4);
    }
}
