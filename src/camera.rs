//! The "🎥 Puppet Camera" filter: put on a Puppet source, its Zoom, Centre
//! and Turn around sliders move the camera. A filter so that Theater Scene's
//! Studio shows its sliders in a source's filter part, its poses save them and
//! ease them from one pose to the next, like every filter's (Puppet Place's
//! twin, for the camera). The picture itself goes through untouched.
//!
//! Its values go on the source's own camera settings (adding the filter never
//! makes the view jump): Zoom multiplies, Centre and Turn add. Several add up;
//! disabled or removed: no effect.

use std::ffi::{CStr, c_char};
use std::sync::Mutex;

use crate::obs::{self, Ptr};

const ID: &CStr = c"theater_puppet_camera_filter";
const NAME: &CStr = c"\u{1F3A5} Puppet Camera";

const S_ZOOM: &CStr = c"zoom";
const S_CENTRE: &CStr = c"centre";
const S_ORBIT: &CStr = c"orbit";

/// What a filter does to its Puppet's camera
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Times the source's zoom (1: as it is)
    pub zoom: f32,
    /// Added to its centre (feet -1 .. head 1)
    pub centre: f32,
    /// Degrees added to its turn around the avatar
    pub orbit: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { zoom: 1.0, centre: 0.0, orbit: 0.0 }
    }
}

/// Each Puppet Camera filter (OBS's pointer): its settings, and the Puppet
/// it's on (found at its tick; 0 before, or while disabled)
static FILTERS: Mutex<Vec<(usize, Camera, usize)>> = Mutex::new(Vec::new());

/// What the enabled Camera filters on `source` do (several add up)
pub fn camera(source: Ptr) -> Camera {
    combined(&FILTERS.lock().unwrap(), source as usize)
}

fn combined(filters: &[(usize, Camera, usize)], source: usize) -> Camera {
    let mut out = Camera::default();
    for (_, c, _) in filters.iter().filter(|(_, _, p)| *p == source) {
        out.zoom *= c.zoom;
        out.centre += c.centre;
        out.orbit += c.orbit;
    }
    out
}

/// The source's own view with the filters' camera on it
pub fn apply(view: &mut crate::render::View, c: Camera) {
    view.zoom = (view.zoom * c.zoom).clamp(0.01, 500.0);
    view.height += c.centre;
    view.orbit += c.orbit;
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
        (api.obs_data_set_default_double)(settings, S_ZOOM.as_ptr(), 1.0);
        (api.obs_data_set_default_double)(settings, S_CENTRE.as_ptr(), 0.0);
        (api.obs_data_set_default_double)(settings, S_ORBIT.as_ptr(), 0.0);
    }
}

unsafe extern "C" fn get_properties(_: Ptr) -> Ptr {
    let api = obs::api();
    unsafe {
        let props = (api.obs_properties_create)();
        (api.obs_properties_add_float_slider)(props, S_ZOOM.as_ptr(), c"Zoom (x: 1 as it is, 2 twice as close)".as_ptr(), 0.1, 10.0, 0.01);
        (api.obs_properties_add_float_slider)(props, S_CENTRE.as_ptr(), c"Centre (toward the feet - / the head +)".as_ptr(), -2.0, 2.0, 0.01);
        (api.obs_properties_add_float_slider)(props, S_ORBIT.as_ptr(), c"Turn around the avatar (degrees)".as_ptr(), -180.0, 180.0, 1.0);
        (api.obs_properties_add_text)(
            props,
            c"camera_info".as_ptr(),
            c"On the Puppet's own camera settings (Zoom times it, Centre and Turn added). Theater Scene's poses save and ease these like any filter's.".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        props
    }
}

/// The filter's OBS source (its `data`: it needs nothing else)
unsafe extern "C" fn create(settings: Ptr, filter: Ptr) -> Ptr {
    FILTERS.lock().unwrap().push((filter as usize, Camera::default(), 0));
    unsafe { update(filter, settings) };
    filter
}

unsafe extern "C" fn destroy(filter: Ptr) {
    FILTERS.lock().unwrap().retain(|(f, _, _)| *f != filter as usize);
}

unsafe extern "C" fn update(filter: Ptr, settings: Ptr) {
    let api = obs::api();
    let get = |k: &CStr| unsafe { (api.obs_data_get_double)(settings, k.as_ptr()) as f32 };
    let c = Camera { zoom: get(S_ZOOM).max(0.01), centre: get(S_CENTRE), orbit: get(S_ORBIT) };
    if let Some(entry) = FILTERS.lock().unwrap().iter_mut().find(|(f, _, _)| *f == filter as usize) {
        entry.1 = c;
    }
}

/// Finds the Puppet it's on (0 while disabled: it does nothing)
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
    fn cameras_on_a_puppet_add_up_and_others_dont_count() {
        let a = Camera { zoom: 2.0, centre: 0.25, orbit: 30.0 };
        let b = Camera { zoom: 1.5, centre: -0.1, orbit: -10.0 };
        let filters = vec![(1, a, 100), (2, b, 100), (3, Camera { zoom: 9.0, ..Camera::default() }, 200), (4, Camera { zoom: 9.0, ..Camera::default() }, 0)];
        let c = combined(&filters, 100);
        assert!((c.zoom - 3.0).abs() < 1e-6 && (c.centre - 0.15).abs() < 1e-6 && (c.orbit - 20.0).abs() < 1e-6, "{c:?}");
        // None (or disabled: on no Puppet): as it is
        assert_eq!(combined(&filters, 999), Camera::default());
        let mut view = crate::render::View { zoom: 1.5, height: 0.2, yaw: 0.0, orbit: 10.0, orthographic: false, position: glam::Vec3::ZERO, light: crate::render::View::LIGHT, light_color: [1.0; 3] };
        apply(&mut view, c);
        assert!((view.zoom - 4.5).abs() < 1e-5 && (view.height - 0.35).abs() < 1e-5 && (view.orbit - 30.0).abs() < 1e-5);
    }
}
