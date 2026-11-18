//! What Puppet costs, written to the log every 10 seconds: the CPU time of
//! its tick (pose, springs, skinning) and of its render (sending the draws),
//! and the GPU time of its render, measured by the GPU itself (libobs's
//! gs_timer: timestamp queries, read a few frames later).
//!
//! ```text
//! ⏱ Theater Puppet, per frame: tick 0.62 ms, render 0.18 ms (CPU); GPU 0.95 ms
//! ```

use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
}

type Ptr = *mut c_void;

/// libobs's GPU timers (None: this OBS hasn't them)
struct Timers {
    create: unsafe extern "C" fn() -> Ptr,
    begin: unsafe extern "C" fn(Ptr),
    end: unsafe extern "C" fn(Ptr),
    data: unsafe extern "C" fn(Ptr, *mut u64) -> bool,
    range_create: unsafe extern "C" fn() -> Ptr,
    range_begin: unsafe extern "C" fn(Ptr),
    range_end: unsafe extern "C" fn(Ptr),
    range_data: unsafe extern "C" fn(Ptr, *mut bool, *mut u64) -> bool,
}

fn timers() -> Option<Timers> {
    let module = unsafe { GetModuleHandleW("obs.dll\0".encode_utf16().collect::<Vec<u16>>().as_ptr()) };
    if module.is_null() {
        return None;
    }
    fn get<T>(module: *mut c_void, name: &CStr) -> Option<T> {
        let p = unsafe { GetProcAddress(module, name.as_ptr()) };
        (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
    }
    Some(Timers {
        create: get(module, c"gs_timer_create")?,
        begin: get(module, c"gs_timer_begin")?,
        end: get(module, c"gs_timer_end")?,
        data: get(module, c"gs_timer_get_data")?,
        range_create: get(module, c"gs_timer_range_create")?,
        range_begin: get(module, c"gs_timer_range_begin")?,
        range_end: get(module, c"gs_timer_range_end")?,
        range_data: get(module, c"gs_timer_range_get_data")?,
    })
}

/// A frame's GPU measure in flight: its range (the clock) and its timer
struct Pending {
    range: Ptr,
    timer: Ptr,
}

// Graphics objects, used on OBS's graphics thread only
unsafe impl Send for Pending {}

#[derive(Default)]
struct State {
    since: Option<Instant>,
    frames: u32,
    tick: Duration,
    render: Duration,
    gpu_ns: f64,
    gpu_frames: u32,
    /// Frames being measured, oldest first (read once the GPU is done)
    pending: Vec<Pending>,
    /// Spare timer pairs (OBS has no way to free one outside the graphics thread; they're reused)
    spare: Vec<Pending>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static TIMERS: std::sync::OnceLock<Option<Timers>> = std::sync::OnceLock::new();

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(s.get_or_insert_with(State::default))
}

/// The tick took `d`
pub fn tick(d: Duration) {
    with(|s| s.tick += d);
}

/// Starts measuring a render on the GPU (graphics thread); `end_render` ends it
pub fn begin_render() -> Option<Instant> {
    let t = TIMERS.get_or_init(timers).as_ref();
    if let Some(t) = t {
        with(|s| {
            let pair = s.spare.pop().unwrap_or_else(|| unsafe { Pending { range: (t.range_create)(), timer: (t.create)() } });
            if !pair.range.is_null() && !pair.timer.is_null() {
                unsafe {
                    (t.range_begin)(pair.range);
                    (t.begin)(pair.timer);
                }
                s.pending.push(pair);
            }
        });
    }
    Some(Instant::now())
}

/// Ends the render begun at `started`; reads the GPU's finished measures,
/// and writes the averages every 10 seconds
pub fn end_render(started: Option<Instant>) {
    let Some(started) = started else { return };
    let t = TIMERS.get_or_init(timers).as_ref();
    with(|s| {
        s.render += started.elapsed();
        s.frames += 1;
        if let (Some(t), Some(last)) = (t, s.pending.last()) {
            unsafe {
                (t.end)(last.timer);
                (t.range_end)(last.range);
            }
        }
        // The GPU's measures of earlier frames, once ready (in order)
        if let Some(t) = t {
            while let Some(first) = s.pending.first() {
                // The newest stays at least a frame
                if s.pending.len() < 3 {
                    break;
                }
                let (mut ticks, mut disjoint, mut frequency) = (0u64, false, 0u64);
                let ready = unsafe { (t.range_data)(first.range, &mut disjoint, &mut frequency) && (t.data)(first.timer, &mut ticks) };
                if !ready {
                    break;
                }
                if !disjoint && frequency > 0 {
                    s.gpu_ns += ticks as f64 * 1e9 / frequency as f64;
                    s.gpu_frames += 1;
                }
                let done = s.pending.remove(0);
                s.spare.push(done);
            }
        }
        let since = *s.since.get_or_insert_with(Instant::now);
        if since.elapsed() >= Duration::from_secs(10) && s.frames > 0 {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0 / s.frames as f64;
            let gpu = if s.gpu_frames > 0 { format!("GPU {:.2} ms", s.gpu_ns / 1e6 / s.gpu_frames as f64) } else { "GPU not measured".into() };
            crate::log(&format!("[Puppet] ⏱ Theater Puppet, per frame ({} frames): tick {:.2} ms, render {:.2} ms (CPU); {gpu}", s.frames, ms(s.tick), ms(s.render)));
            s.since = Some(Instant::now());
            s.frames = 0;
            s.tick = Duration::ZERO;
            s.render = Duration::ZERO;
            s.gpu_ns = 0.0;
            s.gpu_frames = 0;
        }
    });
}
