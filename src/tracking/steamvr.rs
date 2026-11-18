//! SteamVR: the headset, the controllers (and the fingers on them) and the
//! trackers, through
//! SteamVR's own library (openvr_api.dll, in SteamVR's folder), as a
//! background program: it follows SteamVR when it runs, and never starts it.
//!
//! SteamVR's room: metres, Y up, the floor at 0, facing wherever you were
//! told to. Where you stand and face when tracking starts becomes the
//! avatar's place and its forward (turn SteamVR off and on in the Tracking
//! window to set it again).

use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use glam::{Mat3, Quat, Vec3};

use super::dll::Library;
use super::{Body, Hand, Hands, Source, Target, config::Config};

/// vr::TrackedDevicePose_t
#[repr(C)]
#[derive(Clone, Copy)]
struct DevicePose {
    /// Device to room: 3 rows of 4
    matrix: [[f32; 4]; 3],
    velocity: [f32; 3],
    angular_velocity: [f32; 3],
    result: i32,
    valid: bool,
    connected: bool,
}

const MAX_DEVICES: usize = 64;
const APPLICATION_OVERLAY: i32 = 2;
const APPLICATION_BACKGROUND: i32 = 3;
const ORIGIN_STANDING: i32 = 1;
const CLASS_TRACKER: i32 = 3;
const ROLE_LEFT_HAND: i32 = 1;
const ROLE_RIGHT_HAND: i32 = 2;
const PROP_CONTROLLER_TYPE: i32 = 7000;
const EVENT_QUIT: u32 = 700;
const NO_DEVICE: u32 = 0xFFFF_FFFF;

// IVRSystem_023's function table: the places of the ones used
const GET_POSES: usize = 11;
const INDEX_FOR_ROLE: usize = 17;
const DEVICE_CLASS: usize = 19;
const STRING_PROPERTY: usize = 27;
const POLL_EVENT: usize = 29;
const ACKNOWLEDGE_QUIT: usize = 44;
const TABLE_LENGTH: usize = 47;

type Init = unsafe extern "system" fn(*mut i32, i32) -> u32;
type Shutdown = unsafe extern "system" fn();
type Interface = unsafe extern "system" fn(*const c_char, *mut i32) -> *const c_void;
type GetPoses = unsafe extern "system" fn(i32, f32, *mut DevicePose, u32);
type IndexForRole = unsafe extern "system" fn(i32) -> u32;
type DeviceClass = unsafe extern "system" fn(u32) -> i32;
type StringProperty = unsafe extern "system" fn(u32, i32, *mut c_char, u32, *mut i32) -> u32;
type PollEvent = unsafe extern "system" fn(*mut [u8; 64], u32) -> bool;
type AcknowledgeQuit = unsafe extern "system" fn();

// IVRInput_010's function table: the fingers (SteamVR's hand skeletons)
const SET_MANIFEST: usize = 0;
const ACTION_SET: usize = 1;
const ACTION: usize = 2;
const UPDATE_ACTIONS: usize = 4;
const SKELETAL_DATA: usize = 9;
const SKELETAL_SUMMARY: usize = 18;
const INPUT_TABLE_LENGTH: usize = 32;
/// An overlay's action set at this priority gets input whatever app has focus
const PRIORITY_OVERLAY_GLOBAL: i32 = 0x0100_0000;
/// The fingers as the hand's animation has them (works for every controller)
const SUMMARY_FROM_ANIMATION: i32 = 0;

type SetManifest = unsafe extern "system" fn(*const c_char) -> i32;
type Handle = unsafe extern "system" fn(*const c_char, *mut u64) -> i32;
type UpdateActions = unsafe extern "system" fn(*mut ActiveActionSet, u32, u32) -> i32;
type SkeletalData = unsafe extern "system" fn(u64, *mut SkeletalActionData, u32) -> i32;
type SkeletalSummary = unsafe extern "system" fn(u64, i32, *mut SkeletalSummaryData) -> i32;

/// vr::VRActiveActionSet_t
#[repr(C)]
struct ActiveActionSet {
    set: u64,
    restricted_to_device: u64,
    secondary: u64,
    padding: u32,
    priority: i32,
}

/// vr::InputSkeletalActionData_t
#[repr(C)]
#[derive(Default)]
struct SkeletalActionData {
    active: bool,
    origin: u64,
}

/// vr::VRSkeletalSummaryData_t: each finger's curl (thumb first), 0 straight
/// to 1 closed; the splay between neighbours
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SkeletalSummaryData {
    curl: [f32; 5],
    splay: [f32; 4],
}

/// The controller types Puppet binds the hand skeletons for (SteamVR's own
/// names, from its drivers' input profiles)
const CONTROLLERS: [&str; 10] = [
    "knuckles",
    "knuckles_ev1",
    "frame_controller",
    "oculus_touch",
    "vive_controller",
    "vive_cosmos_controller",
    "vive_focus3_controller",
    "holographic_controller",
    "hpmotioncontroller",
    "pico_controller",
];

/// Writes the action manifest the fingers need (plugin_config/
/// theater-puppet/steamvr): two skeleton actions, bound for every
/// controller in CONTROLLERS
fn write_manifest() -> Option<PathBuf> {
    let dir = crate::config_dir().join("steamvr");
    std::fs::create_dir_all(&dir).ok()?;
    let binding = serde_json::json!({
        "bindings": { "/actions/puppet": {
            "skeleton": [
                { "output": "/actions/puppet/in/left_hand", "path": "/user/hand/left/input/skeleton/left" },
                { "output": "/actions/puppet/in/right_hand", "path": "/user/hand/right/input/skeleton/right" }
            ],
            "sources": [], "poses": [], "haptics": []
        } }
    });
    let mut defaults = Vec::new();
    for controller in CONTROLLERS {
        let mut b = binding.clone();
        b["controller_type"] = controller.into();
        let file = format!("bindings_{controller}.json");
        std::fs::write(dir.join(&file), serde_json::to_string_pretty(&b).ok()?).ok()?;
        defaults.push(serde_json::json!({ "controller_type": controller, "binding_url": file }));
    }
    let manifest = serde_json::json!({
        "default_bindings": defaults,
        "actions": [
            { "name": "/actions/puppet/in/left_hand", "type": "skeleton", "skeleton": "/skeleton/hand/left" },
            { "name": "/actions/puppet/in/right_hand", "type": "skeleton", "skeleton": "/skeleton/hand/right" }
        ],
        "action_sets": [{ "name": "/actions/puppet", "usage": "leftright" }],
        "localization": [{
            "language_tag": "en_US",
            "/actions/puppet": "Theater Puppet",
            "/actions/puppet/in/left_hand": "Left hand (fingers)",
            "/actions/puppet/in/right_hand": "Right hand (fingers)"
        }]
    });
    let path = dir.join("actions.json");
    std::fs::write(&path, serde_json::to_string_pretty(&manifest).ok()?).ok()?;
    Some(path)
}

/// A hand's fingers from SteamVR's summary of them
fn fingers(s: &SkeletalSummaryData) -> Hand {
    hand_from_curls(s.curl, Some(s.splay))
}

/// A hand's fingers from each one's curl (thumb first, 0 straight to 1
/// closed): each joint bent its share of a fist, the thumb less; the splay
/// between neighbours (0 together, 1 wide apart), when known, as spread
pub(super) fn hand_from_curls(curl: [f32; 5], splay: Option<[f32; 4]>) -> Hand {
    let mut hand = Hand::default();
    // A closed fist, radians at each joint: thumb, then the four fingers
    const THUMB: [f32; 3] = [0.6, 0.7, 0.8];
    const FINGER: [f32; 3] = [1.5, 1.7, 1.1];
    for f in 0..5 {
        let curl = curl[f].clamp(0.0, 1.0);
        let full = if f == 0 { THUMB } else { FINGER };
        hand.bend[f] = full.map(|b| b * curl);
    }
    // Splay: 0 together, 1 wide apart (about 0.2 at rest). Each finger moves
    // away from the middle one by half the gaps on its side opening up
    let Some(splay) = splay else { return hand };
    let open = splay.map(|v| (v.clamp(0.0, 1.0) - 0.2) * 0.35);
    hand.spread[1] = -open[1];
    hand.spread[3] = open[2] * 0.5;
    hand.spread[4] = open[2] * 0.5 + open[3];
    hand
}

/// The fingers' actions in SteamVR, once their manifest is in
struct Fingers {
    update: UpdateActions,
    data: SkeletalData,
    summary: SkeletalSummary,
    set: u64,
    hands: [u64; 2],
}

impl Fingers {
    /// Sets up the fingers' actions in a SteamVR session (None: SteamVR
    /// refused; the controllers still work)
    fn new(input: &[*const c_void; INPUT_TABLE_LENGTH]) -> Option<Fingers> {
        let manifest = write_manifest()?;
        let (set_manifest, handle_of): (SetManifest, Handle) = unsafe { (std::mem::transmute(input[SET_MANIFEST]), std::mem::transmute(input[ACTION])) };
        let set_handle: Handle = unsafe { std::mem::transmute(input[ACTION_SET]) };
        let path = std::ffi::CString::new(manifest.to_string_lossy().into_owned()).ok()?;
        let error = unsafe { set_manifest(path.as_ptr()) };
        if error != 0 {
            crate::log(&format!("[Puppet] ⚠️ SteamVR: the fingers' action manifest was refused (error {error})"));
            return None;
        }
        let mut set = 0u64;
        let mut hands = [0u64; 2];
        unsafe {
            set_handle(c"/actions/puppet".as_ptr(), &mut set);
            handle_of(c"/actions/puppet/in/left_hand".as_ptr(), &mut hands[0]);
            handle_of(c"/actions/puppet/in/right_hand".as_ptr(), &mut hands[1]);
        }
        if set == 0 || hands.contains(&0) {
            return None;
        }
        unsafe {
            Some(Fingers {
                update: std::mem::transmute(input[UPDATE_ACTIONS]),
                data: std::mem::transmute(input[SKELETAL_DATA]),
                summary: std::mem::transmute(input[SKELETAL_SUMMARY]),
                set,
                hands,
            })
        }
    }

    /// Both hands' fingers now (None for a hand whose controller has none)
    fn read(&self) -> Hands {
        let mut active = ActiveActionSet { set: self.set, restricted_to_device: 0, secondary: 0, padding: 0, priority: PRIORITY_OVERLAY_GLOBAL };
        unsafe { (self.update)(&mut active, std::mem::size_of::<ActiveActionSet>() as u32, 1) };
        let hand = |action: u64| {
            let mut data = SkeletalActionData::default();
            let ok = unsafe { (self.data)(action, &mut data, std::mem::size_of::<SkeletalActionData>() as u32) } == 0;
            if !ok || !data.active {
                return None;
            }
            let mut summary = SkeletalSummaryData::default();
            (unsafe { (self.summary)(action, SUMMARY_FROM_ANIMATION, &mut summary) } == 0).then(|| fingers(&summary))
        };
        Hands { left: hand(self.hands[0]), right: hand(self.hands[1]) }
    }
}

/// Where SteamVR keeps its library: the runtime named in openvrpaths.vrpath
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(bin) = crate::data_dir().and_then(|d| Some(d.parent()?.join("bin").join("64bit"))) {
        out.push(bin.join("openvr_api.dll"));
    }
    let paths = std::env::var("LOCALAPPDATA").map(|l| PathBuf::from(l).join("openvr").join("openvrpaths.vrpath"));
    let json = paths.ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    for runtime in json.as_ref().and_then(|j| j.get("runtime")).and_then(|r| r.as_array()).into_iter().flatten() {
        if let Some(dir) = runtime.as_str() {
            out.push(PathBuf::from(dir).join("bin").join("win64").join("openvr_api.dll"));
        }
    }
    out
}

pub(super) fn rotation(m: &[[f32; 4]; 3]) -> Quat {
    Quat::from_mat3(&Mat3::from_cols(Vec3::new(m[0][0], m[1][0], m[2][0]), Vec3::new(m[0][1], m[1][1], m[2][1]), Vec3::new(m[0][2], m[1][2], m[2][2]))).normalize()
}

fn position(m: &[[f32; 4]; 3]) -> Vec3 {
    Vec3::new(m[0][3], m[1][3], m[2][3])
}

/// From SteamVR's room to the avatar's space: where the user stood and
/// faced when tracking started is the avatar's place, facing +Z
#[derive(Clone, Copy, Debug)]
pub(super) struct Room {
    turn: Quat,
    /// Where the user stood (the floor under the headset), in the room
    origin: Vec3,
}

impl Room {
    /// From the headset's pose: it looks along its own -Z
    pub(super) fn from_headset(m: &[[f32; 4]; 3]) -> Room {
        let forward = rotation(m) * Vec3::NEG_Z;
        // Turned about Y so that forward becomes +Z
        let turn = Quat::from_rotation_y(-forward.x.atan2(forward.z));
        let p = position(m);
        Room { turn, origin: Vec3::new(p.x, 0.0, p.z) }
    }

    fn place(&self, p: Vec3) -> Vec3 {
        self.turn * (p - self.origin)
    }

    /// The head: its turn is from looking straight ahead (a headset looks
    /// along -Z, the avatar along +Z: half a turn between them)
    pub(super) fn head(&self, m: &[[f32; 4]; 3]) -> Target {
        Target { position: self.place(position(m)), rotation: (self.turn * rotation(m) * Quat::from_rotation_y(std::f32::consts::PI)).normalize() }
    }

    /// A hand holding a controller: fingers along it (its -Z), the palm
    /// against its side (facing inward), the wrist a little behind
    pub(super) fn hand(&self, m: &[[f32; 4]; 3], left: bool) -> Target {
        let r = self.turn * rotation(m);
        let palm = if left { Vec3::X } else { Vec3::NEG_X };
        Target { position: self.place(position(m)) + r * Vec3::new(0.0, 0.0, 0.07), rotation: crate::ik::hand_turn(left, r * Vec3::NEG_Z, r * palm) }
    }
}

/// A tracker strapped on: its turn is from how it was when tracking started
#[derive(Clone, Copy)]
pub(super) struct Strapped {
    pub(super) device: u32,
    pub(super) at_start: Quat,
}

impl Strapped {
    pub(super) fn target(&self, room: &Room, m: &[[f32; 4]; 3]) -> Target {
        Target { position: room.place(position(m)), rotation: (room.turn * rotation(m) * self.at_start.inverse() * room.turn.inverse()).normalize() }
    }
}

/// What VR_Init's error means for the user (OpenVR's EVRInitError)
fn init_error(code: i32) -> &'static str {
    match code {
        100 | 101 | 102 => "SteamVR isn't installed properly",
        108 | 126 => "no headset found: connect it (or its streaming app), then start SteamVR",
        121 => "SteamVR isn't running: start it",
        117 | 115 | 119 => "SteamVR is starting or closing",
        137 | 138 | 139 => "the headset is busy (rebooting or updating)",
        _ => "SteamVR can't be reached",
    }
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn restart(config: &Config) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !config.steamvr {
        return;
    }
    let _ = std::thread::Builder::new().name("theater-puppet-steamvr".into()).spawn(move || {
        let alive = || GENERATION.load(Ordering::SeqCst) == generation;
        let wait = |seconds: u64| {
            for _ in 0..seconds * 4 {
                if !alive() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        };
        let Some((lib, path)) = Library::first(&candidates()) else {
            super::set_status(Source::SteamVr, "SteamVR isn't installed (no openvr_api.dll)");
            return crate::log("[Puppet] ⚠️ SteamVR: no openvr_api.dll found: SteamVR isn't installed where openvrpaths.vrpath says");
        };
        let (Some(init), Some(shutdown), Some(interface)) =
            (lib.function::<Init>(c"VR_InitInternal"), lib.function::<Shutdown>(c"VR_ShutdownInternal"), lib.function::<Interface>(c"VR_GetGenericInterface"))
        else {
            super::set_status(Source::SteamVr, "SteamVR's library isn't one Puppet knows");
            return crate::log(&format!("[Puppet] ⚠️ SteamVR: {} hasn't OpenVR's functions", path.display()));
        };
        crate::log(&format!("[Puppet] 🥽 SteamVR: using {}", path.display()));
        let mut told = false;
        while alive() {
            // Follows SteamVR once it runs (a background program doesn't start it)
            let mut error = 0;
            unsafe { init(&mut error, APPLICATION_BACKGROUND) };
            // Running: again as an overlay, which may read the controllers'
            // fingers whatever app has focus (a background program can't)
            let mut overlay = false;
            if error == 0 {
                unsafe { shutdown() };
                unsafe { init(&mut error, APPLICATION_OVERLAY) };
                overlay = error == 0;
                if !overlay {
                    unsafe { init(&mut error, APPLICATION_BACKGROUND) };
                }
            }
            if error != 0 {
                let why = init_error(error);
                super::set_status(Source::SteamVr, why);
                if !told {
                    told = true;
                    crate::log(&format!("[Puppet] 🥽 SteamVR: {why} (error {error}); trying again every 3 seconds"));
                }
                wait(3);
                continue;
            }
            let table = unsafe { interface(c"FnTable:IVRSystem_023".as_ptr(), &mut error) } as *const [*const c_void; TABLE_LENGTH];
            if table.is_null() || error != 0 {
                unsafe { shutdown() };
                super::set_status(Source::SteamVr, "this SteamVR is too old for Puppet: update it");
                return crate::log(&format!("[Puppet] ⚠️ SteamVR: no IVRSystem_023 (error {error})"));
            }
            let f = unsafe { &*table };
            let (get_poses, index_for_role, device_class, string_property, poll_event, acknowledge_quit): (GetPoses, IndexForRole, DeviceClass, StringProperty, PollEvent, AcknowledgeQuit) = unsafe {
                (
                    std::mem::transmute(f[GET_POSES]),
                    std::mem::transmute(f[INDEX_FOR_ROLE]),
                    std::mem::transmute(f[DEVICE_CLASS]),
                    std::mem::transmute(f[STRING_PROPERTY]),
                    std::mem::transmute(f[POLL_EVENT]),
                    std::mem::transmute(f[ACKNOWLEDGE_QUIT]),
                )
            };
            let input = unsafe { interface(c"FnTable:IVRInput_010".as_ptr(), &mut error) } as *const [*const c_void; INPUT_TABLE_LENGTH];
            let fingers = if overlay && !input.is_null() && error == 0 { Fingers::new(unsafe { &*input }) } else { None };
            crate::log(&format!("[Puppet] 🥽 SteamVR: connected{}", if fingers.is_some() { ", with the fingers" } else { " (no fingers: SteamVR wouldn't give them)" }));
            super::set_status(Source::SteamVr, "connected, waiting for the headset");
            told = false;
            let mut room: Option<Room> = None;
            // The trackers by what they're strapped to: hips, left foot, right foot
            let mut strapped: [Option<Strapped>; 3] = [None; 3];
            let mut looked = std::time::Instant::now() - Duration::from_secs(10);
            let mut poses = [DevicePose { matrix: [[0.0; 4]; 3], velocity: [0.0; 3], angular_velocity: [0.0; 3], result: 0, valid: false, connected: false }; MAX_DEVICES];
            'tracking: while alive() {
                // SteamVR closing: let go of it, and wait for the next one
                let mut event = [0u8; 64];
                while unsafe { poll_event(&mut event, 64) } {
                    if u32::from_le_bytes([event[0], event[1], event[2], event[3]]) == EVENT_QUIT {
                        unsafe { acknowledge_quit() };
                        crate::log("[Puppet] 🥽 SteamVR: closed");
                        break 'tracking;
                    }
                }
                unsafe { get_poses(ORIGIN_STANDING, 0.0, poses.as_mut_ptr(), MAX_DEVICES as u32) };
                let pose = |device: u32| poses.get(device as usize).filter(|p| p.valid && p.connected).map(|p| p.matrix);
                // Device 0 is the headset
                let Some(headset) = pose(0) else {
                    super::set_status(Source::SteamVr, "connected, the headset isn't tracked");
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                };
                let room = *room.get_or_insert_with(|| Room::from_headset(&headset));
                // Which tracker is where: looked up now and then (they come and go)
                if looked.elapsed() > Duration::from_secs(2) {
                    looked = std::time::Instant::now();
                    for device in 1..MAX_DEVICES as u32 {
                        if unsafe { device_class(device) } != CLASS_TRACKER {
                            continue;
                        }
                        let mut name = [0 as c_char; 128];
                        let mut error = 0;
                        unsafe { string_property(device, PROP_CONTROLLER_TYPE, name.as_mut_ptr(), name.len() as u32, &mut error) };
                        let name = unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().to_lowercase();
                        let slot = if name.contains("waist") || name.contains("hip") {
                            0
                        } else if name.contains("left_foot") {
                            1
                        } else if name.contains("right_foot") {
                            2
                        } else {
                            continue;
                        };
                        if strapped[slot].is_none_or(|s| s.device != device) {
                            if let Some(m) = pose(device) {
                                strapped[slot] = Some(Strapped { device, at_start: rotation(&m) });
                                crate::log(&format!("[Puppet] 🥽 SteamVR: tracker {device} is the {}", ["hips", "left foot", "right foot"][slot]));
                            }
                        }
                    }
                }
                let hand = |role: i32, left: bool| {
                    let device = unsafe { index_for_role(role) };
                    if device == NO_DEVICE { None } else { pose(device).map(|m| room.hand(&m, left)) }
                };
                let tracker = |slot: usize| strapped[slot].and_then(|s| pose(s.device).map(|m| s.target(&room, &m)));
                let body = Body {
                    head: Some(room.head(&headset)),
                    left_hand: hand(ROLE_LEFT_HAND, true),
                    right_hand: hand(ROLE_RIGHT_HAND, false),
                    hips: tracker(0),
                    left_foot: tracker(1),
                    right_foot: tracker(2),
                    left_elbow: None,
                    right_elbow: None,
                };
                let hands = body.left_hand.is_some() as u32 + body.right_hand.is_some() as u32;
                let trackers = strapped.iter().flatten().count();
                let curled = fingers.as_ref().map(|f| f.read()).unwrap_or_default();
                let with_fingers = curled.left.is_some() as u32 + curled.right.is_some() as u32;
                super::set_status(Source::SteamVr, format!("receiving the headset, {hands} controller(s) ({with_fingers} with fingers), {trackers} tracker(s)"));
                super::set_body(Source::SteamVr, body);
                if with_fingers > 0 {
                    super::set_hands(Source::SteamVr, curled);
                }
                // As often as a headset shows a picture
                std::thread::sleep(Duration::from_millis(11));
            }
            unsafe { shutdown() };
            wait(3);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device at `p`, turned `yaw` degrees about Y from looking along -Z
    fn device(p: Vec3, yaw: f32) -> [[f32; 4]; 3] {
        let m = Mat3::from_quat(Quat::from_rotation_y(yaw.to_radians()));
        let c = m.to_cols_array_2d();
        [[c[0][0], c[1][0], c[2][0], p.x], [c[0][1], c[1][1], c[2][1], p.y], [c[0][2], c[1][2], c[2][2], p.z]]
    }

    #[test]
    fn the_structure_is_openvrs_size() {
        assert_eq!(std::mem::size_of::<DevicePose>(), 80);
        assert_eq!(std::mem::size_of::<ActiveActionSet>(), 32);
        assert_eq!(std::mem::size_of::<SkeletalActionData>(), 16);
        assert_eq!(std::mem::size_of::<SkeletalSummaryData>(), 36);
    }

    #[test]
    fn curls_bend_the_fingers() {
        let open = fingers(&SkeletalSummaryData { curl: [0.0; 5], splay: [0.2; 4] });
        assert!(open.bend.iter().flatten().all(|b| *b == 0.0) && open.spread.iter().all(|s| s.abs() < 1e-6));
        // A point: the index straight, the others closed
        let point = fingers(&SkeletalSummaryData { curl: [1.0, 0.0, 1.0, 1.0, 1.0], splay: [0.2; 4] });
        assert!(point.bend[1].iter().all(|b| *b == 0.0));
        assert!(point.bend[2][0] > 1.4 && point.bend[4][1] > 1.6 && point.bend[0][2] > 0.7);
        // Wide apart: the index and the little finger go out
        let wide = fingers(&SkeletalSummaryData { curl: [0.0; 5], splay: [1.0; 4] });
        assert!(wide.spread[1] < -0.2 && wide.spread[4] > 0.3);
    }

    #[test]
    fn where_you_stand_is_the_avatars_place() {
        // Standing at (2, 0, 1) in the room, the eyes at 1.6 m, facing 50 degrees off
        let start = device(Vec3::new(2.0, 1.6, 1.0), 50.0);
        let room = Room::from_headset(&start);
        let head = room.head(&start);
        assert!((head.position - Vec3::new(0.0, 1.6, 0.0)).length() < 1e-4, "{:?}", head.position);
        assert!(head.rotation.angle_between(Quat::IDENTITY) < 1e-3);

        // A step forward (the way the headset looks) is +Z for the avatar
        let forward = Quat::from_rotation_y(50f32.to_radians()) * Vec3::NEG_Z;
        let ahead = room.head(&device(Vec3::new(2.0, 1.6, 1.0) + forward * 0.5, 50.0));
        assert!((ahead.position - Vec3::new(0.0, 1.6, 0.5)).length() < 1e-4, "{:?}", ahead.position);

        // Looking 30 degrees to your left turns the avatar's head to its left (+X)
        let looking = room.head(&device(Vec3::new(2.0, 1.6, 1.0), 80.0));
        let nose = looking.rotation * Vec3::Z;
        assert!(nose.x > 0.45 && nose.z > 0.8, "{nose:?}");
    }

    #[test]
    fn a_controller_is_a_hand() {
        let start = device(Vec3::new(0.0, 1.6, 0.0), 0.0);
        let room = Room::from_headset(&start);
        // The left controller: to your left (-X in a room you face along -Z), pointing ahead
        let hand = room.hand(&device(Vec3::new(-0.3, 1.2, -0.4), 0.0), true);
        assert!(hand.position.x > 0.25 && hand.position.z > 0.3 && hand.position.z < 0.4, "{:?}", hand.position);
        let (fingers, palm) = (hand.rotation * Vec3::X, hand.rotation * Vec3::NEG_Y);
        // Fingers ahead, the palm facing the avatar's right (inward)
        assert!(fingers.z > 0.99 && palm.x < -0.99, "{fingers:?} {palm:?}");
    }

    #[test]
    fn a_tracker_turns_from_how_it_started() {
        let start = device(Vec3::new(0.0, 1.6, 0.0), 0.0);
        let room = Room::from_headset(&start);
        // Strapped on any way round
        let strapped = Strapped { device: 3, at_start: rotation(&device(Vec3::ZERO, 137.0)) };
        let still = strapped.target(&room, &device(Vec3::new(0.0, 1.0, 0.0), 137.0));
        assert!(still.rotation.angle_between(Quat::IDENTITY) < 1e-3);
        // The hips turned 20 degrees to the left
        let turned = strapped.target(&room, &device(Vec3::new(0.0, 1.0, 0.0), 157.0));
        let front = turned.rotation * Vec3::Z;
        assert!(front.x > 0.3 && front.z > 0.9, "{front:?}");
    }
}
