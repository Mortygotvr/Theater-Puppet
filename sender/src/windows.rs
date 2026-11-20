//! Windows: Theater Puppet's own readers (its tracking/desk.rs and
//! gamepad.rs: GetAsyncKeyState and a mouse hook; XInput or SDL), run
//! outside OBS.

use theater_puppet::tracking::{desk, gamepad};

use crate::state::{Desk, Pad};

pub struct Input {
    keyboard: bool,
    controller: bool,
}

/// Puppet's state as the sender's (the same fields: through their JSON)
fn same<A: serde::Serialize, B: serde::de::DeserializeOwned>(a: A) -> Option<B> {
    serde_json::to_value(a).ok().and_then(|v| serde_json::from_value(v).ok())
}

impl Input {
    pub fn start(keyboard: bool, controller: bool) -> Input {
        theater_puppet::OUTSIDE_OBS.store(true, std::sync::atomic::Ordering::Relaxed);
        desk::restart(&desk::Settings { enabled: keyboard });
        gamepad::restart(&gamepad::Settings { enabled: controller, ..Default::default() });
        Input { keyboard, controller }
    }

    pub fn read(&mut self) -> (Option<Desk>, Option<Pad>) {
        let d = if self.keyboard { desk::latest().and_then(same) } else { None };
        let p = if self.controller { gamepad::latest().and_then(same) } else { None };
        (d, p)
    }

    pub fn report(&self) -> Vec<String> {
        theater_puppet::tracking::statuses().into_iter().map(|(source, _, text)| format!("{}: {text}", source.name())).collect()
    }
}
