//! The "🎤 Puppet Lipsync" audio filter: put on the microphone's source (after
//! its noise suppression, if any), it listens to what goes through it (and
//! lets it through untouched) and opens the avatars' mouths to it: by how
//! loud it is, and, with vowels on, in the shape of the vowel heard (A, I,
//! U, E, O, from the voice's first two formants).
//!
//! Its settings are the filter's: intensity, noise gate, smoothing, vowels,
//! the shape key each vowel opens (empty: VRoid's Fcl_MTH_A… when the avatar
//! has them, else ARKit's mouth shapes), and how it goes with a tracked face
//! (motion.rs reads `mouth()`).

use std::ffi::{CStr, c_char};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::obs::{self, Ptr};

const ID: &CStr = c"theater_puppet_lipsync_filter";
const NAME: &CStr = c"\u{1F3A4} Puppet Lipsync";

const S_INTENSITY: &CStr = c"intensity";
const S_GATE: &CStr = c"gate_db";
const S_SMOOTHING: &CStr = c"smoothing";
const S_VOWELS: &CStr = c"vowels";
const S_MODE: &CStr = c"mode";
/// Each vowel's shape key (A, I, U, E, O)
const S_KEYS: [&CStr; 5] = [c"key_a", c"key_i", c"key_u", c"key_e", c"key_o"];
const VOWEL_NAMES: [&str; 5] = ["A", "I", "U", "E", "O"];

/// How the microphone's mouth goes with a face that's tracked
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The more open of the two
    #[default]
    Add,
    /// The microphone's mouth only (the face app keeps the eyes, brows…)
    Replace,
    /// Only when no face is tracked
    Fallback,
}

/// The mouth the microphone hears: how open (0–1, intensity in), which
/// vowel (A, I, U, E, O weights, summing to 1), each vowel's shape key (""
/// for the automatic one)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mouth {
    pub open: f32,
    pub vowels: [f32; 5],
    pub mode: Mode,
    pub keys: [String; 5],
}

/// The newest mouth, and when (from whichever Lipsync filter heard last)
static LATEST: Mutex<Option<(Mouth, Instant)>> = Mutex::new(None);

/// What a Lipsync filter hears (the tests: as if heard now)
#[cfg(test)]
pub fn hear(mouth: Mouth) {
    *LATEST.lock().unwrap() = Some((mouth, Instant::now()));
}

/// The mouth now; None when no Lipsync filter is hearing anything
pub fn mouth() -> Option<Mouth> {
    LATEST.lock().unwrap().as_ref().filter(|(_, at)| at.elapsed() < Duration::from_millis(300)).map(|(m, _)| m.clone())
}

#[derive(Clone)]
struct Settings {
    intensity: f32,
    gate_db: f32,
    smoothing: f32,
    vowels: bool,
    mode: Mode,
    keys: [String; 5],
}

/// One filter: its settings, the last samples heard, its smoothed mouth
struct Filter {
    settings: Mutex<Settings>,
    state: Mutex<State>,
}

struct State {
    samples: Vec<f32>,
    open: f32,
    vowels: [f32; 5],
}

/// How many samples a look takes (about 20 ms at 48 kHz)
const WINDOW: usize = 1024;

// ---------------------------------------------------------------------------
// The sound

/// The loudness of `x`, decibels
pub fn level_db(x: &[f32]) -> f32 {
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    20.0 * rms.max(1e-9).log10()
}

/// The strength of frequency `hz` in `x` (Goertzel), `rate` samples a second
fn power(x: &[f32], hz: f32, rate: f32) -> f32 {
    let w = std::f32::consts::TAU * hz / rate;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for (i, &v) in x.iter().enumerate() {
        // A Hann window: the edges don't smear the peaks
        let h = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (x.len() as f32 - 1.0)).cos();
        let s = v * h + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - c * s1 * s2
}

/// The strongest frequency between `lo` and `hi` Hz
fn peak(x: &[f32], lo: f32, hi: f32, rate: f32) -> f32 {
    let mut best = (lo, 0.0);
    let mut hz = lo;
    while hz <= hi {
        let p = power(x, hz, rate);
        if p > best.1 {
            best = (hz, p);
        }
        hz += 40.0;
    }
    best.0
}

/// The vowel heard (A, I, U, E, O weights) from the first two formants:
/// the nearest of each vowel's typical pair weighs most
pub fn vowels(x: &[f32], rate: f32) -> [f32; 5] {
    let f1 = peak(x, 250.0, 950.0, rate);
    let f2 = peak(x, 700.0, 2600.0, rate).max(f1 + 150.0);
    // (F1, F2), Hz: A, I, U, E, O
    const VOWEL: [(f32, f32); 5] = [(750.0, 1250.0), (300.0, 2250.0), (330.0, 850.0), (480.0, 1850.0), (520.0, 900.0)];
    let near: Vec<f32> = VOWEL
        .iter()
        .map(|(v1, v2)| {
            // F1 counts more (F2 is wider apart between vowels)
            let d = ((f1 - v1) / 150.0).powi(2) + ((f2 - v2) / 400.0).powi(2);
            (-d).exp()
        })
        .collect();
    let sum: f32 = near.iter().sum::<f32>().max(1e-6);
    std::array::from_fn(|i| near[i] / sum)
}

/// How open the mouth is for a loudness: closed at the gate, wide open 30 dB
/// above it
fn openness(db: f32, gate_db: f32) -> f32 {
    ((db - gate_db) / 30.0).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// The filter

/// libobs's obs_audio_data: planes of float samples
#[repr(C)]
pub struct AudioData {
    data: [*mut u8; 8],
    frames: u32,
    timestamp: u64,
}

/// libobs's obs_audio_info
#[repr(C)]
struct AudioInfo {
    samples_per_sec: u32,
    speakers: u32,
}

/// obs_source_info up to filter_audio (it follows video_render)
#[repr(C)]
struct AudioFilterInfo {
    base: obs::SourceInfo,
    filter_video: Option<unsafe extern "C" fn(Ptr, Ptr) -> Ptr>,
    filter_audio: Option<unsafe extern "C" fn(Ptr, *mut AudioData) -> *mut AudioData>,
}
unsafe impl Sync for AudioFilterInfo {}

/// OBS's sample rate
fn rate() -> f32 {
    type GetAudioInfo = unsafe extern "C" fn(*mut AudioInfo) -> bool;
    static RATE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *RATE.get_or_init(|| {
        let get = crate::tracking::obs_function::<GetAudioInfo>(c"obs_get_audio_info");
        let mut info = AudioInfo { samples_per_sec: 0, speakers: 0 };
        match get {
            Some(f) if unsafe { f(&mut info) } && info.samples_per_sec > 0 => info.samples_per_sec as f32,
            _ => 48000.0,
        }
    })
}

pub fn register() {
    static INFO: AudioFilterInfo = AudioFilterInfo {
        base: obs::SourceInfo {
            id: ID.as_ptr(),
            type_: obs::OBS_SOURCE_TYPE_FILTER,
            output_flags: obs::OBS_SOURCE_AUDIO,
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
            video_tick: None,
            video_render: None,
        },
        filter_video: None,
        filter_audio: Some(filter_audio),
    };
    unsafe { (obs::api().obs_register_source_s)(&INFO.base, std::mem::size_of::<AudioFilterInfo>()) };
}

unsafe extern "C" fn get_name(_: Ptr) -> *const c_char {
    NAME.as_ptr()
}

unsafe extern "C" fn get_defaults(settings: Ptr) {
    let api = obs::api();
    unsafe {
        (api.obs_data_set_default_double)(settings, S_INTENSITY.as_ptr(), 1.0);
        (api.obs_data_set_default_double)(settings, S_GATE.as_ptr(), -45.0);
        (api.obs_data_set_default_double)(settings, S_SMOOTHING.as_ptr(), 0.5);
        (api.obs_data_set_default_bool)(settings, S_VOWELS.as_ptr(), true);
        (api.obs_data_set_default_string)(settings, S_MODE.as_ptr(), c"add".as_ptr());
    }
}

unsafe extern "C" fn get_properties(_: Ptr) -> Ptr {
    let api = obs::api();
    unsafe {
        let props = (api.obs_properties_create)();
        (api.obs_properties_add_float_slider)(props, S_INTENSITY.as_ptr(), c"Intensity (how wide the mouth opens)".as_ptr(), 0.0, 3.0, 0.05);
        (api.obs_properties_add_float_slider)(props, S_GATE.as_ptr(), c"Noise gate (dB): quieter than this, the mouth stays shut".as_ptr(), -80.0, -10.0, 1.0);
        (api.obs_properties_add_float_slider)(props, S_SMOOTHING.as_ptr(), c"Smoothing (0 = jumpy, 1 = soft)".as_ptr(), 0.0, 1.0, 0.05);
        (api.obs_properties_add_bool)(props, S_VOWELS.as_ptr(), c"Vowels (A, I, U, E, O shapes; off: it only opens)".as_ptr());
        let list = (api.obs_properties_add_list)(props, S_MODE.as_ptr(), c"With a tracked face (iFacialMocap…)".as_ptr(), obs::OBS_COMBO_TYPE_LIST, obs::OBS_COMBO_FORMAT_STRING);
        for (label, value) in [
            (c"Add (the more open of the two)", c"add"),
            (c"Replace the mouth (the face keeps eyes, brows…)", c"replace"),
            (c"Only when no face is tracked", c"fallback"),
        ] {
            (api.obs_property_list_add_string)(list, label.as_ptr(), value.as_ptr());
        }
        for (i, key) in S_KEYS.iter().enumerate() {
            let label = std::ffi::CString::new(format!("Shape key for {} (empty: automatic)", VOWEL_NAMES[i])).unwrap_or_default();
            (api.obs_properties_add_text)(props, key.as_ptr(), label.as_ptr(), obs::OBS_TEXT_DEFAULT);
        }
        (api.obs_properties_add_text)(props, c"lipsync_info".as_ptr(), c"Every Theater Puppet hears it. The sound goes through untouched.".as_ptr(), obs::OBS_TEXT_INFO);
        props
    }
}

fn read(settings: Ptr) -> Settings {
    let api = obs::api();
    let mode = {
        let p = unsafe { (api.obs_data_get_string)(settings, S_MODE.as_ptr()) };
        let s = if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
        match s.as_str() {
            "replace" => Mode::Replace,
            "fallback" => Mode::Fallback,
            _ => Mode::Add,
        }
    };
    unsafe {
        Settings {
            intensity: (api.obs_data_get_double)(settings, S_INTENSITY.as_ptr()) as f32,
            gate_db: (api.obs_data_get_double)(settings, S_GATE.as_ptr()) as f32,
            smoothing: (api.obs_data_get_double)(settings, S_SMOOTHING.as_ptr()) as f32,
            vowels: (api.obs_data_get_bool)(settings, S_VOWELS.as_ptr()),
            mode,
            keys: S_KEYS.map(|k| {
                let p = (api.obs_data_get_string)(settings, k.as_ptr());
                if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().trim().to_string() }
            }),
        }
    }
}

unsafe extern "C" fn create(settings: Ptr, _source: Ptr) -> Ptr {
    let filter = Box::new(Filter {
        settings: Mutex::new(read(settings)),
        state: Mutex::new(State { samples: Vec::with_capacity(WINDOW * 2), open: 0.0, vowels: [1.0, 0.0, 0.0, 0.0, 0.0] }),
    });
    Box::into_raw(filter) as Ptr
}

unsafe extern "C" fn destroy(data: Ptr) {
    drop(unsafe { Box::from_raw(data as *mut Filter) });
}

unsafe extern "C" fn update(data: Ptr, settings: Ptr) {
    let filter = unsafe { &*(data as *const Filter) };
    *filter.settings.lock().unwrap() = read(settings);
}

/// The sound going through: heard, let through as it is
unsafe extern "C" fn filter_audio(data: Ptr, audio: *mut AudioData) -> *mut AudioData {
    if data.is_null() || audio.is_null() {
        return audio;
    }
    let filter = unsafe { &*(data as *const Filter) };
    let a = unsafe { &*audio };
    let plane = a.data[0] as *const f32;
    if plane.is_null() || a.frames == 0 {
        return audio;
    }
    let samples = unsafe { std::slice::from_raw_parts(plane, a.frames as usize) };
    let settings = filter.settings.lock().unwrap().clone();
    let mut state = filter.state.lock().unwrap();
    state.samples.extend_from_slice(samples);
    let excess = state.samples.len().saturating_sub(WINDOW);
    state.samples.drain(..excess);
    if state.samples.len() < WINDOW / 2 {
        return audio;
    }
    let dt = a.frames as f32 / rate();
    let target = openness(level_db(&state.samples), settings.gate_db);
    // Opens fast, closes a little slower; smoothing slows both
    let speed = if target > state.open { 30.0 } else { 15.0 } * (1.0 - settings.smoothing.clamp(0.0, 0.95));
    let k = (dt * speed).min(1.0);
    state.open += (target - state.open) * k;
    if settings.vowels && target > 0.0 {
        let heard = vowels(&state.samples, rate());
        for i in 0..5 {
            state.vowels[i] += (heard[i] - state.vowels[i]) * k;
        }
    } else if !settings.vowels {
        state.vowels = [1.0, 0.0, 0.0, 0.0, 0.0];
    }
    let mouth = Mouth { open: (state.open * settings.intensity).clamp(0.0, 1.0), vowels: state.vowels, mode: settings.mode, keys: settings.keys };
    *LATEST.lock().unwrap() = Some((mouth, Instant::now()));
    audio
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vowel-like sound: a voice at `pitch` with peaks at f1 and f2
    fn voice(f1: f32, f2: f32) -> Vec<f32> {
        let rate = 48000.0;
        (0..WINDOW)
            .map(|i| {
                let t = i as f32 / rate;
                let tone = |hz: f32, a: f32| a * (std::f32::consts::TAU * hz * t).sin();
                tone(140.0, 0.05) + tone(f1, 0.3) + tone(f2, 0.2)
            })
            .collect()
    }

    #[test]
    fn vowels_are_told_apart() {
        let best = |w: [f32; 5]| w.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(best(vowels(&voice(750.0, 1250.0), 48000.0)), 0, "A");
        assert_eq!(best(vowels(&voice(300.0, 2250.0), 48000.0)), 1, "I");
        assert_eq!(best(vowels(&voice(330.0, 850.0), 48000.0)), 2, "U");
        assert_eq!(best(vowels(&voice(480.0, 1850.0), 48000.0)), 3, "E");
        assert_eq!(best(vowels(&voice(560.0, 950.0), 48000.0)), 4, "O");
    }

    #[test]
    fn loudness_opens_the_mouth_above_the_gate() {
        let quiet = vec![0.0005f32; WINDOW];
        let loud = voice(750.0, 1250.0);
        assert_eq!(openness(level_db(&quiet), -45.0), 0.0);
        assert!(openness(level_db(&loud), -45.0) > 0.5);
    }
}
