//! The "🧍 Puppet Place" filter: put on a Puppet source, its Turn and
//! Position sliders turn and move the avatar. It's a filter so that Theater
//! Scene's Studio shows its sliders in a source's filter part, its poses save
//! them and ease them from one pose to the next, like every filter's. The
//! picture itself goes through untouched.
//!
//! Its values add to the source's own Turn and Position settings (adding
//! the filter never makes the avatar jump). Disabled or removed: no effect.

use std::ffi::{CStr, c_char};
use std::sync::Mutex;

use glam::Vec3;

use crate::obs::{self, Ptr};

const ID: &CStr = c"theater_puppet_place_filter";
const NAME: &CStr = c"\u{1F9CD} Puppet Place";

const S_TURN: &CStr = c"turn";
const S_POSITION: [&CStr; 3] = [c"x", c"y", c"z"];

/// Turn's range, degrees; position's, metres (x, y, z)
pub const TURN: f64 = 180.0;
pub const RANGE: [f64; 3] = [3.0, 2.0, 3.0];

/// What a filter adds to its Puppet
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Offset {
    /// Degrees
    pub turn: f32,
    /// Metres: right, up, towards the camera
    pub position: Vec3,
}

/// Each Puppet Place filter (OBS's pointer): its settings, and the Puppet
/// it's on (found at its tick; 0 before)
static FILTERS: Mutex<Vec<(usize, Offset, usize)>> = Mutex::new(Vec::new());

/// What the enabled Place filters on `source` add (several add up)
pub fn offset(source: Ptr) -> Offset {
    let filters = FILTERS.lock().unwrap();
    let mut out = Offset::default();
    for (_, o, _) in filters.iter().filter(|(_, _, p)| *p == source as usize) {
        out.turn += o.turn;
        out.position += o.position;
    }
    out
}

pub fn register() {
    static INFO: obs::SourceInfo = obs::SourceInfo {
        id: ID.as_ptr(),
        type_: obs::OBS_SOURCE_TYPE_FILTER,
        output_flags: obs::OBS_SOURCE_VIDEO,
        get_name: Some(get_name),
        create: Some(create),
        destroy: Some(destroy),
        get_width: None,
        get_height: None,
        get_defaults: Some(get_defaults),
        get_properties: Some(get_properties),
        update: Some(update),
        activate: None,
        deactivate: None,
        show: None,
        hide: None,
        video_tick: Some(video_tick),
        video_render: Some(video_render),
    };
    unsafe { (obs::api().obs_register_source_s)(&INFO, std::mem::size_of::<obs::SourceInfo>()) };
}

unsafe extern "C" fn get_name(_: Ptr) -> *const c_char {
    NAME.as_ptr()
}

unsafe extern "C" fn get_defaults(settings: Ptr) {
    let api = obs::api();
    unsafe {
        (api.obs_data_set_default_double)(settings, S_TURN.as_ptr(), 0.0);
        for k in S_POSITION {
            (api.obs_data_set_default_double)(settings, k.as_ptr(), 0.0);
        }
    }
}

unsafe extern "C" fn get_properties(_: Ptr) -> Ptr {
    let api = obs::api();
    unsafe {
        let props = (api.obs_properties_create)();
        (api.obs_properties_add_float_slider)(props, S_TURN.as_ptr(), c"Turn (degrees)".as_ptr(), -TURN, TURN, 1.0);
        for (i, label) in [c"Left / right (m)", c"Down / up (m)", c"Back / forward (m)"].into_iter().enumerate() {
            (api.obs_properties_add_float_slider)(props, S_POSITION[i].as_ptr(), label.as_ptr(), -RANGE[i], RANGE[i], 0.01);
        }
        (api.obs_properties_add_text)(
            props,
            c"place_info".as_ptr(),
            c"Added to the Puppet's own Turn and Position. Theater Scene's poses save and ease these like any filter's.".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        props
    }
}

/// The filter's OBS source (its `data`: it needs nothing else)
unsafe extern "C" fn create(settings: Ptr, filter: Ptr) -> Ptr {
    FILTERS.lock().unwrap().push((filter as usize, Offset::default(), 0));
    unsafe { update(filter, settings) };
    filter
}

unsafe extern "C" fn destroy(filter: Ptr) {
    FILTERS.lock().unwrap().retain(|(f, _, _)| *f != filter as usize);
}

unsafe extern "C" fn update(filter: Ptr, settings: Ptr) {
    let api = obs::api();
    let get = |k: &CStr| unsafe { (api.obs_data_get_double)(settings, k.as_ptr()) as f32 };
    let o = Offset { turn: get(S_TURN), position: Vec3::new(get(S_POSITION[0]), get(S_POSITION[1]), get(S_POSITION[2])) };
    if let Some(entry) = FILTERS.lock().unwrap().iter_mut().find(|(f, _, _)| *f == filter as usize) {
        entry.1 = o;
    }
}

/// Finds the Puppet it's on (0 while disabled: it adds nothing)
unsafe extern "C" fn video_tick(filter: Ptr, _dt: f32) {
    let api = obs::api();
    let parent = unsafe { if (api.obs_source_enabled)(filter) { (api.obs_filter_get_parent)(filter) as usize } else { 0 } };
    if let Some(entry) = FILTERS.lock().unwrap().iter_mut().find(|(f, _, _)| *f == filter as usize) {
        entry.2 = parent;
    }
}

/// The picture as it is
unsafe extern "C" fn video_render(filter: Ptr, _effect: Ptr) {
    unsafe { (obs::api().obs_source_skip_video_filter)(filter) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_on_a_puppet_add_up_and_others_dont_count() {
        let (a, b) = (0x1000 as Ptr, 0x2000 as Ptr);
        {
            let mut f = FILTERS.lock().unwrap();
            f.push((1, Offset { turn: 30.0, position: Vec3::X }, a as usize));
            f.push((2, Offset { turn: 15.0, position: Vec3::Y }, a as usize));
            f.push((3, Offset { turn: 90.0, position: Vec3::Z }, b as usize));
            f.push((4, Offset { turn: 90.0, position: Vec3::Z }, 0));
        }
        assert_eq!(offset(a), Offset { turn: 45.0, position: Vec3::new(1.0, 1.0, 0.0) });
        assert_eq!(offset(0x3000 as Ptr), Offset::default());
        FILTERS.lock().unwrap().retain(|(f, _, _)| *f > 4);
    }
}
