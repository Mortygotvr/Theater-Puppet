// Theater Puppet's tracking dock: calibrate the Leap, reset MediaPipe.
// A Theater Scene Rhai dock; it asks Puppet through its OBS procedure
// theater_puppet_tracking (src/tracking/dock.rs).
const ID = "theater_puppet_tracking";
const NAME = "🎯 Puppet Tracking";
const KIND = "dock";

fn init() {
    this.note = "";
}

fn ui(ui) {
    let s = call_proc("theater_puppet_tracking", #{ action: "get" });
    if type_of(s) != "map" || !("leap" in s) {
        ui.label("Theater Puppet isn't loaded.");
        ui.repaint_after(2.0);
        return;
    }

    ui.heading("🥽 VR telemetry");
    ui.small(s.telemetry_status);
    if s.telemetry {
        ui.label(`T-pose: ${s.telemetry_calibration}`);
        let scale = ui.slider("Scale (reach)", s.telemetry_scale, 0.5, 3.0);
        if scale != s.telemetry_scale {
            call_proc("theater_puppet_tracking", #{ action: "set", telemetry_scale: scale });
        }
        ui.row();
        if ui.button("Calibrate (press both triggers)") {
            call_proc("theater_puppet_tracking", #{ action: "telemetry_calibrate", now: false });
        }
        if ui.button("Now") {
            call_proc("theater_puppet_tracking", #{ action: "telemetry_calibrate", now: true });
        }
        if ui.button("Reset") {
            call_proc("theater_puppet_tracking", #{ action: "telemetry_reset" });
        }
        ui.end();
        ui.small("Stand in a T-pose, arms out, palms down, looking ahead.");
    }

    ui.separator();
    ui.heading("🖐 Leap Motion");
    ui.small(s.leap_status);
    if s.leap {
        let height = ui.slider("Height (m)", s.leap_height, -0.6, 0.6);
        let forward = ui.slider("Forward (m)", s.leap_forward, -0.6, 0.6);
        if height != s.leap_height || forward != s.leap_forward {
            call_proc("theater_puppet_tracking", #{ action: "set", leap_height: height, leap_forward: forward });
        }
        ui.row();
        if ui.button("Centre on my hands now") {
            let r = call_proc("theater_puppet_tracking", #{ action: "leap_centre" });
            this.note = if r.done { "Centred: hands where they are now rest in front of the avatar." } else { "No hands seen: hold them over the Leap, then click." };
        }
        if ui.button("Reset") {
            call_proc("theater_puppet_tracking", #{ action: "set", leap_height: 0.0, leap_forward: 0.0 });
            this.note = "";
        }
        ui.end();
        if this.note != "" {
            ui.small(this.note);
        }
    }

    ui.separator();
    ui.heading("📱 iFacialMocap");
    ui.small(s.ifacialmocap_status);
    if s.ifacialmocap {
        if ui.button("Re-centre head") {
            call_proc("theater_puppet_tracking", #{ action: "ifacialmocap_recentre" });
        }
        ui.small("Sit as you normally do, then click (head position: assign it to iFacialMocap in the Tracking window).");
    }

    ui.separator();
    ui.heading("📷 MediaPipe");
    ui.small(s.mediapipe_status);
    if s.mediapipe {
        ui.row();
        if ui.button("Re-centre head") {
            call_proc("theater_puppet_tracking", #{ action: "mediapipe_recentre" });
        }
        if ui.button("Restart") {
            call_proc("theater_puppet_tracking", #{ action: "mediapipe_restart" });
        }
        ui.end();
        ui.small("Re-centre: sit as you normally do, then click.");
    }

    // The statuses change by themselves
    ui.repaint_after(0.5);
}
