//! A look at what Puppet draws, for checking it from outside OBS: create
//! `snapshot.request` in its config folder, and the next frame is saved as
//! `snapshot.png` there, and the next body parts map as `parts.png` (the
//! request file is removed).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::obs::{self, Ptr};

static WANTED: AtomicBool = AtomicBool::new(false);
/// The next parts map is saved too (parts.png)
static PARTS_WANTED: AtomicBool = AtomicBool::new(false);

/// True once per request
pub fn parts_wanted() -> bool {
    PARTS_WANTED.swap(false, Ordering::SeqCst)
}
static LAST_CHECK: Mutex<Option<Instant>> = Mutex::new(None);

/// Looks for the request file, at most once a second (from video_tick)
pub fn poll() {
    let mut last = LAST_CHECK.lock().unwrap();
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 1.0) {
        return;
    }
    *last = Some(Instant::now());
    let request = crate::config_dir().join("snapshot.request");
    if request.exists() {
        let _ = std::fs::remove_file(&request);
        WANTED.store(true, Ordering::SeqCst);
        PARTS_WANTED.store(true, Ordering::SeqCst);
    }
}

/// True once per request
pub fn wanted() -> bool {
    WANTED.swap(false, Ordering::SeqCst)
}

/// Copies `texture` (RGBA, width x height) off the GPU and saves it as a PNG
/// on a thread. Call on the graphics thread.
pub fn save(texture: Ptr, width: u32, height: u32) {
    let api = obs::api();
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    unsafe {
        let stage = (api.gs_stagesurface_create)(width, height, obs::GS_RGBA);
        if stage.is_null() {
            return;
        }
        (api.gs_stage_texture)(stage, texture);
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut linesize = 0u32;
        if (api.gs_stagesurface_map)(stage, &mut data, &mut linesize) {
            let row = (width * 4) as usize;
            for y in 0..height as usize {
                let src = std::slice::from_raw_parts(data.add(y * linesize as usize), row);
                pixels[y * row..(y + 1) * row].copy_from_slice(src);
            }
            (api.gs_stagesurface_unmap)(stage);
        }
        (api.gs_stagesurface_destroy)(stage);
    }
    std::thread::spawn(move || {
        let path = crate::config_dir().join("snapshot.png");
        match image::save_buffer(&path, &pixels, width, height, image::ExtendedColorType::Rgba8) {
            Ok(()) => crate::log(&format!("[Puppet] 📸 Snapshot saved: {}", path.display())),
            Err(e) => crate::log(&format!("[Puppet] ⚠️ Snapshot: {e}")),
        }
    });
}
