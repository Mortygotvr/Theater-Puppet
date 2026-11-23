//! The "🎥 Puppet Camera" filter: every Puppet source has one (added by
//! itself when the source loads, again if it's removed). Its sliders are the
//! whole camera: Zoom, the projection, Centre, Turn around, Position, and a
//! Spin that turns the model on itself like a display stand. A filter so that
//! Theater Scene's Studio shows its sliders in a source's filter part, and its
//! poses save them and ease them from one pose to the next, like every
//! filter's. The picture itself goes through untouched.
//!
//! Its values go on the source's own camera settings (the filter never makes
//! the view jump): Zoom multiplies, Centre, Turn and Position add, the
//! projection replaces the source's when it says one. Spin back at 0, the
//! model turns back to face the front (the nearest whole turn). Several add
//! up; disabled: no effect.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;

use glam::Vec3;

use crate::obs::{self, Ptr};

pub const ID: &CStr = c"theater_puppet_camera_filter";
const NAME: &CStr = c"\u{1F3A5} Puppet Camera";

const S_ZOOM: &CStr = c"zoom";
const S_PROJECTION: &CStr = c"projection";
const S_CENTRE: &CStr = c"centre";
const S_ORBIT: &CStr = c"orbit";
const S_POSITION: [&CStr; 3] = [c"x", c"y", c"z"];
const S_SPIN: &CStr = c"spin";

/// What a filter does to its Puppet's camera
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Times the source's zoom (1: as it is)
    pub zoom: f32,
    /// Orthographic (Some(true)), perspective (Some(false)), or as the source says
    pub orthographic: Option<bool>,
    /// Added to its centre (feet -1 .. head 1)
    pub centre: f32,
    /// Degrees added to its turn around the avatar
    pub orbit: f32,
    /// Metres added to where it stands: right (as seen), up, toward the camera
    pub position: Vec3,
    /// Degrees a second the model turns on itself (0: back to the front)
    pub spin: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { zoom: 1.0, orthographic: None, centre: 0.0, orbit: 0.0, position: Vec3::ZERO, spin: 0.0 }
    }
}

/// One filter: its settings, the Puppet it's on (found at its tick; 0
/// before, or while disabled), and how far its spin has taken the camera round
#[derive(Clone, Copy, Debug)]
struct Entry {
    filter: usize,
    camera: Camera,
    parent: usize,
    turned: f32,
}

static FILTERS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// What the enabled Camera filters on `source` do (several add up), and the
/// degrees their spins have taken the camera round
pub fn camera(source: Ptr) -> (Camera, f32) {
    combined(&FILTERS.lock().unwrap(), source as usize)
}

fn combined(filters: &[Entry], source: usize) -> (Camera, f32) {
    let mut out = Camera::default();
    let mut turned = 0.0;
    for e in filters.iter().filter(|e| e.parent == source) {
        let c = e.camera;
        out.zoom *= c.zoom;
        out.centre += c.centre;
        out.orbit += c.orbit;
        out.position += c.position;
        out.orthographic = c.orthographic.or(out.orthographic);
        turned += e.turned;
    }
    (out, turned)
}

/// The source's own view with the filters' camera on it
pub fn apply(view: &mut crate::render::View, (c, turned): (Camera, f32)) {
    view.zoom = (view.zoom * c.zoom).clamp(0.01, 500.0);
    view.height += c.centre;
    view.orbit += c.orbit;
    view.position += c.position;
    if let Some(o) = c.orthographic {
        view.orthographic = o;
    }
    // The camera going round the model (the spin)
    view.orbit += turned;
}

/// A spin `dt` seconds on: the camera going round at `spin` degrees a second;
/// at 0 it stays where it got to (it doesn't go back)
fn spun(turned: f32, spin: f32, dt: f32) -> f32 {
    (turned + spin * dt).rem_euclid(360.0)
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

// ---------------------------------------------------------------------------
// Every Puppet has one: added when it's missing (on OBS's UI thread)

/// The Puppets a Camera filter is being added to (asked once)
static ADDING: Mutex<Vec<usize>> = Mutex::new(Vec::new());

unsafe extern "C" fn found_one(_parent: Ptr, child: Ptr, found: *mut c_void) {
    let id = unsafe { (obs::api().obs_source_get_unversioned_id)(child) };
    if !id.is_null() && unsafe { CStr::from_ptr(id) } == ID {
        unsafe { *(found as *mut bool) = true };
    }
}

fn has_one(source: Ptr) -> bool {
    let mut found = false;
    unsafe { (obs::api().obs_source_enum_filters)(source, found_one, &mut found as *mut bool as *mut c_void) };
    found
}

/// `source` (a Puppet) gets its Camera filter if it has none
pub fn ensure(source: Ptr) {
    if source.is_null() || has_one(source) {
        return;
    }
    let mut adding = ADDING.lock().unwrap();
    if adding.contains(&(source as usize)) {
        return;
    }
    let api = obs::api();
    // Kept alive until the task has run
    let held = unsafe { (api.obs_source_get_ref)(source) };
    if held.is_null() {
        return;
    }
    adding.push(source as usize);
    unsafe { (api.obs_queue_task)(obs::OBS_TASK_UI, add_task, held, false) };
}

unsafe extern "C" fn add_task(source: *mut c_void) {
    let api = obs::api();
    unsafe {
        if !has_one(source) {
            let filter = (api.obs_source_create)(ID.as_ptr(), NAME.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut());
            if !filter.is_null() {
                (api.obs_source_filter_add)(source, filter);
                (api.obs_source_release)(filter);
                crate::log("[Puppet] 🎥 Puppet Camera filter added to a Puppet source");
            }
        }
        ADDING.lock().unwrap().retain(|s| *s != source as usize);
        (api.obs_source_release)(source);
    }
}

// ---------------------------------------------------------------------------

unsafe extern "C" fn get_name(_: Ptr) -> *const c_char {
    NAME.as_ptr()
}

unsafe extern "C" fn get_defaults(settings: Ptr) {
    let api = obs::api();
    unsafe {
        (api.obs_data_set_default_double)(settings, S_ZOOM.as_ptr(), 1.0);
        (api.obs_data_set_default_string)(settings, S_PROJECTION.as_ptr(), c"".as_ptr());
        for k in [S_CENTRE, S_ORBIT, S_SPIN] {
            (api.obs_data_set_default_double)(settings, k.as_ptr(), 0.0);
        }
        for k in S_POSITION {
            (api.obs_data_set_default_double)(settings, k.as_ptr(), 0.0);
        }
    }
}

unsafe extern "C" fn get_properties(_: Ptr) -> Ptr {
    let api = obs::api();
    unsafe {
        let props = (api.obs_properties_create)();
        (api.obs_properties_add_float_slider)(props, S_ZOOM.as_ptr(), c"Zoom (x: 1 as it is, 2 twice as close)".as_ptr(), 0.1, 10.0, 0.01);
        let projection = (api.obs_properties_add_list)(props, S_PROJECTION.as_ptr(), c"Camera".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
        (api.obs_property_list_add_string)(projection, c"As the source".as_ptr(), c"".as_ptr());
        (api.obs_property_list_add_string)(projection, c"Perspective".as_ptr(), c"perspective".as_ptr());
        (api.obs_property_list_add_string)(projection, c"Orthographic (no perspective)".as_ptr(), c"orthographic".as_ptr());
        (api.obs_properties_add_float_slider)(props, S_CENTRE.as_ptr(), c"Centre (toward the feet - / the head +)".as_ptr(), -2.0, 2.0, 0.01);
        (api.obs_properties_add_float_slider)(props, S_ORBIT.as_ptr(), c"Turn around the avatar (degrees)".as_ptr(), -180.0, 180.0, 1.0);
        for (key, label, range) in [
            (S_POSITION[0], c"Position: left / right (m)", 3.0),
            (S_POSITION[1], c"Position: down / up (m)", 2.0),
            (S_POSITION[2], c"Position: back / forward (m)", 3.0),
        ] {
            (api.obs_properties_add_float_slider)(props, key.as_ptr(), label.as_ptr(), -range, range, 0.01);
        }
        (api.obs_properties_add_float_slider)(props, S_SPIN.as_ptr(), c"Spin: the camera goes round the model, degrees a second (0: it stops where it is)".as_ptr(), -180.0, 180.0, 1.0);
        (api.obs_properties_add_text)(
            props,
            c"camera_info".as_ptr(),
            c"On the Puppet's own camera settings (Zoom times it, the others added). Theater Scene's poses save and ease these like any filter's. Every Puppet has this filter (it comes back if removed).".as_ptr(),
            obs::OBS_TEXT_INFO,
        );
        props
    }
}

/// The filter's OBS source (its `data`: it needs nothing else)
unsafe extern "C" fn create(settings: Ptr, filter: Ptr) -> Ptr {
    FILTERS.lock().unwrap().push(Entry { filter: filter as usize, camera: Camera::default(), parent: 0, turned: 0.0 });
    unsafe { update(filter, settings) };
    filter
}

unsafe extern "C" fn destroy(filter: Ptr) {
    FILTERS.lock().unwrap().retain(|e| e.filter != filter as usize);
}

unsafe extern "C" fn update(filter: Ptr, settings: Ptr) {
    let api = obs::api();
    let get = |k: &CStr| unsafe { (api.obs_data_get_double)(settings, k.as_ptr()) as f32 };
    let projection = unsafe { CStr::from_ptr((api.obs_data_get_string)(settings, S_PROJECTION.as_ptr())) }.to_bytes();
    let c = Camera {
        zoom: get(S_ZOOM).max(0.01),
        orthographic: match projection {
            b"orthographic" => Some(true),
            b"perspective" => Some(false),
            _ => None,
        },
        centre: get(S_CENTRE),
        orbit: get(S_ORBIT),
        position: Vec3::new(get(S_POSITION[0]), get(S_POSITION[1]), get(S_POSITION[2])),
        spin: get(S_SPIN),
    };
    if let Some(e) = FILTERS.lock().unwrap().iter_mut().find(|e| e.filter == filter as usize) {
        e.camera = c;
    }
}

/// Finds the Puppet it's on (0 while disabled: it does nothing); its spin a frame on
unsafe extern "C" fn video_tick(filter: Ptr, dt: f32) {
    let api = obs::api();
    let parent = unsafe { if (api.obs_source_enabled)(filter) { (api.obs_filter_get_parent)(filter) as usize } else { 0 } };
    if let Some(e) = FILTERS.lock().unwrap().iter_mut().find(|e| e.filter == filter as usize) {
        e.parent = parent;
        e.turned = spun(e.turned, e.camera.spin, dt.clamp(0.0, 0.1));
    }
}

/// The picture as it is
unsafe extern "C" fn video_render(filter: Ptr, _effect: Ptr) {
    unsafe { (obs::api().obs_source_skip_video_filter)(filter) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(filter: usize, camera: Camera, parent: usize, turned: f32) -> Entry {
        Entry { filter, camera, parent, turned }
    }

    #[test]
    fn cameras_on_a_puppet_add_up_and_others_dont_count() {
        let a = Camera { zoom: 2.0, centre: 0.25, orbit: 30.0, position: Vec3::new(0.5, 0.0, 0.0), orthographic: Some(true), spin: 0.0 };
        let b = Camera { zoom: 1.5, centre: -0.1, orbit: -10.0, ..Camera::default() };
        let filters = vec![entry(1, a, 100, 90.0), entry(2, b, 100, 0.0), entry(3, Camera { zoom: 9.0, ..Camera::default() }, 200, 0.0), entry(4, Camera { zoom: 9.0, ..Camera::default() }, 0, 0.0)];
        let (c, turned) = combined(&filters, 100);
        assert!((c.zoom - 3.0).abs() < 1e-6 && (c.centre - 0.15).abs() < 1e-6 && (c.orbit - 20.0).abs() < 1e-6, "{c:?}");
        assert_eq!((c.orthographic, c.position, turned), (Some(true), Vec3::new(0.5, 0.0, 0.0), 90.0));
        // None (or disabled: on no Puppet): as it is
        assert_eq!(combined(&filters, 999), (Camera::default(), 0.0));
        let mut view = crate::render::View { zoom: 1.5, height: 0.2, yaw: 5.0, orbit: 10.0, orthographic: false, position: Vec3::ZERO, light: crate::render::View::LIGHT, light_color: [1.0; 3] };
        apply(&mut view, (c, turned));
        // The orbits (30) and the spins' turn (90) both take the camera round; the model stays
        assert!((view.zoom - 4.5).abs() < 1e-5 && (view.height - 0.35).abs() < 1e-5 && (view.orbit - 120.0).abs() < 1e-5);
        assert!(view.orthographic && (view.yaw - 5.0).abs() < 1e-5 && view.position.x == 0.5);
    }

    #[test]
    fn the_spin_goes_round_and_stops_where_it_is() {
        let mut t = 0.0;
        for _ in 0..60 {
            t = spun(t, 90.0, 1.0 / 60.0);
        }
        assert!((t - 90.0).abs() < 1e-3, "a second at 90 a second: {t}");
        // Spin 0: it stays there
        for _ in 0..600 {
            t = spun(t, 0.0, 1.0 / 60.0);
        }
        assert!((t - 90.0).abs() < 1e-3, "it went back: {t}");
        // Round and round: no jump at a whole turn; backwards too
        let mut u = 350.0;
        u = spun(u, 60.0, 0.5);
        assert!((u - 20.0).abs() < 1e-3, "{u}");
        u = spun(u, -60.0, 1.0);
        assert!((u - 320.0).abs() < 1e-3, "{u}");
        // It moves the camera round (orbit), not the model (yaw)
        let mut view = crate::render::View { zoom: 1.0, height: 0.0, yaw: 5.0, orbit: 10.0, orthographic: false, position: Vec3::ZERO, light: crate::render::View::LIGHT, light_color: [1.0; 3] };
        apply(&mut view, (Camera::default(), 45.0));
        assert!((view.orbit - 55.0).abs() < 1e-5 && (view.yaw - 5.0).abs() < 1e-5, "{} {}", view.orbit, view.yaw);
    }
}
