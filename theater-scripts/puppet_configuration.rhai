// Theater Puppet's Configuration window: every tracking option in one place,
// a tab for each way of tracking. A Theater Scene Rhai window (KIND
// "window": its entry in the Theater menu opens it). It asks Puppet through its
// OBS procedures:
// - theater_puppet_tracking (src/tracking/dock.rs): the tracking settings
//   every Puppet follows (tracking.json), statuses, calibrations;
// - theater_puppet_command (src/remote.rs): one Puppet source's own
//   settings (Follow tracking, the controller's and the desk's place, hands,
//   models), for the tabs with a "Puppet" list.
const ID = "theater_puppet_configuration";
const NAME = "⚙ Puppet Configuration";
const KIND = "window";

fn init() {
    this.tab = "General";
    this.puppet = "";
    this.note = "";
    this.edits = #{};
    this.lists = #{ cameras: [], controllers: [] };
    this.frames = 0;
}

fn tabs() {
    [
        ["General", "🎯 General"], ["IFacialMocap", "📱 iFacialMocap"], ["MeowFace", "🐱 MeowFace"], ["MediaPipe", "📷 MediaPipe"], ["SteamVr", "🥽 SteamVR"],
        ["Telemetry", "📡 VR telemetry"], ["Leap", "🖐 Leap Motion"], ["Controller", "🎮 Controller"], ["Desk", "⌨ Keyboard & mouse"],
    ]
}

fn tracking(args) {
    call_proc("theater_puppet_tracking", args)
}

fn set_cfg(values) {
    tracking(#{ action: "set_config", values: values })
}

/// A choice among [value, label] pairs; the value picked
fn pick(ui, title, value, pairs) {
    let labels = [];
    let current = "";
    for p in pairs {
        labels.push(p[1]);
        if p[0] == value { current = p[1]; }
    }
    let chosen = ui.choice(title, current, labels);
    for p in pairs {
        if p[1] == chosen { return p[0]; }
    }
    value
}

/// A tracking setting's slider (`group`: its object, like "ifacialmocap", or "")
fn cfg_slider(ui, c, key, label, lo, hi) {
    let v = ui.slider(label, c[key], lo, hi);
    if v != c[key] {
        let m = #{};
        m[key] = v;
        set_cfg(m);
    }
}

fn cfg_check(ui, c, key, label) {
    let v = ui.checkbox(label, c[key]);
    if v != c[key] {
        let m = #{};
        m[key] = v;
        set_cfg(m);
    }
}

/// A VR input's feel (`group`: telemetry_feel or steamvr_feel): how much of
/// each part's movement the avatar copies, and its own smoothing
fn feel_sliders(ui, c, group) {
    ui.separator();
    ui.strong("How much of your movement the avatar copies");
    ui.small("1: all of it; 0: none (the idle pose). VR follows every lean and step: lower keeps the avatar calmer.");
    let f = c[group];
    for pair in [["head_turn", "Head turn"], ["head_move", "Head position"], ["hands", "Hands"], ["hips", "Hips"], ["feet", "Feet"], ["smoothing", "Smoothing (on top of General's)"]] {
        let v = ui.slider(pair[1], f[pair[0]], 0.0, 1.0);
        if v != f[pair[0]] {
            let inner = #{};
            inner[pair[0]] = v;
            let m = #{};
            m[group] = inner;
            set_cfg(m);
        }
    }
}

/// An input's on / off (its object's `enabled`, or the setting itself)
fn input_on(ui, c, key, label) {
    let obj = type_of(c[key]) == "map";
    let on = if obj { c[key].enabled } else { c[key] };
    let v = ui.checkbox(label, on);
    if v != on {
        let m = #{};
        m[key] = if obj { #{ enabled: v } } else { v };
        set_cfg(m);
    }
    v
}

/// One Puppet's own settings: a slider, a check, a choice
fn avatar_set(puppet, key, v) {
    let m = #{};
    m[key] = v;
    call_proc("theater_puppet_command", #{ action: "avatar_set", source: puppet, values: m })
}

fn av_slider(ui, puppet, a, key, label, lo, hi) {
    let v = ui.slider(label, a[key], lo, hi);
    if v != a[key] { avatar_set(puppet, key, v); }
}

fn av_check(ui, puppet, a, key, label) {
    let v = ui.checkbox(label, a[key]);
    if v != a[key] { avatar_set(puppet, key, v); }
}

fn av_pick(ui, puppet, a, key, label, pairs) {
    let v = pick(ui, label, a[key], pairs);
    if v != a[key] { avatar_set(puppet, key, v); }
}

fn status_line(ui, s, source) {
    ui.small(`Now: ${s.statuses[source]}`);
}

fn ui(ui) {
    this.frames += 1;
    // The cameras and controllers now and then (asking OBS takes a moment)
    let lists = this.frames % 20 == 1;
    let s = tracking(#{ action: "get", lists: lists });
    if type_of(s) != "map" || !("config" in s) {
        ui.label("Theater Puppet isn't loaded.");
        ui.repaint_after(2.0);
        return;
    }
    if lists {
        this.lists = #{ cameras: s.cameras, controllers: s.controllers };
    }
    let c = s.config;

    // The tabs, in two rows
    let all = tabs();
    for r in [0, 1] {
        ui.row();
        let from = if r == 0 { 0 } else { 5 };
        let to = if r == 0 { 5 } else { all.len() };
        for i in from..to {
            if ui.selectable(all[i][1], this.tab == all[i][0]) {
                this.tab = all[i][0];
                this.note = "";
            }
        }
        ui.end();
    }
    ui.separator();

    // The Puppet sources, for the tabs that change one of them
    let puppets = [];
    let listed = call_proc("theater_puppet_command", #{ action: "list" });
    if type_of(listed) == "map" && "puppets" in listed {
        for p in listed.puppets { puppets.push(p.source); }
    }
    if !puppets.contains(this.puppet) && puppets.len() > 0 {
        this.puppet = puppets[0];
    }
    let avatar = ();
    if ["General", "Controller", "Desk"].contains(this.tab) {
        if puppets.len() == 0 {
            ui.small("No Puppet source yet: add a 🎭 Theater Puppet source.");
        } else {
            if puppets.len() > 1 {
                this.puppet = ui.choice("Puppet", this.puppet, puppets);
            } else {
                ui.small(`Puppet: ${this.puppet}`);
            }
            let got = call_proc("theater_puppet_command", #{ action: "avatar", source: this.puppet });
            if type_of(got) == "map" && "settings" in got { avatar = got.settings; }
        }
    }

    switch this.tab {
        "General" => {
            if type_of(avatar) == "map" {
                av_check(ui, this.puppet, avatar, "tracked", "This Puppet follows tracking");
                ui.separator();
            }
            ui.strong("What tracks what");
            ui.small("For each part of the avatar, the input that drives it. Auto: whichever of its inputs sent last.");
            for p in s.parts {
                let pairs = [["auto", "Auto"]];
                for i in p.inputs { pairs.push(i); }
                pairs.push(["off", "Off"]);
                let v = pick(ui, p.name, p.pick, pairs);
                if v != p.pick {
                    tracking(#{ action: "assign", part: p.key, pick: v });
                }
            }
            ui.separator();
            ui.strong("How it feels");
            cfg_check(ui, c, "mirror", "Mirror (your left moves the avatar's right)");
            cfg_slider(ui, c, "head_strength", "Head turn strength", 0.0, 1.5);
            cfg_slider(ui, c, "spine_lean", "Lean with the spine and hips before the legs bend", 0.0, 1.0);
            cfg_slider(ui, c, "torso_follow", "Torso turns with the head", 0.0, 1.0);
            cfg_slider(ui, c, "smoothing", "Smoothing (0 raw, 1 smooth but a little late)", 0.0, 1.0);
            cfg_slider(ui, c, "user_height", "Your height (metres)", 1.0, 2.2);
        },
        "IFacialMocap" => {
            input_on(ui, c, "ifacialmocap", "iFacialMocap (iPhone)");
            status_line(ui, s, "IFacialMocap");
            this.phone_address(ui, c, "ifacialmocap");
            if ui.button("Re-centre head") {
                tracking(#{ action: "ifacialmocap_recentre" });
            }
            ui.small("Sit as you normally do, then click. Head position: set it to iFacialMocap in General.");
        },
        "MeowFace" => {
            input_on(ui, c, "meowface", "MeowFace (Android)");
            status_line(ui, s, "MeowFace");
            this.phone_address(ui, c, "meowface");
        },
        "MediaPipe" => {
            input_on(ui, c, "mediapipe", "MediaPipe (webcam: body and face)");
            status_line(ui, s, "MediaPipe");
            let pairs = [["", "(none)"]];
            for cam in this.lists.cameras { pairs.push([cam[1], cam[0]]); }
            let cam = pick(ui, "Camera", c.camera, pairs);
            if cam != c.camera { set_cfg(#{ camera: cam }); }
            ui.row();
            if ui.button("Re-centre head") { tracking(#{ action: "mediapipe_recentre" }); }
            if ui.button("Restart") { tracking(#{ action: "mediapipe_restart" }); }
            ui.end();
            ui.small("Re-centre: sit as you normally do, then click.");
        },
        "SteamVr" => {
            input_on(ui, c, "steamvr", "SteamVR (headset, controllers, trackers)");
            status_line(ui, s, "SteamVr");
            feel_sliders(ui, c, "steamvr_feel");
        },
        "Telemetry" => {
            input_on(ui, c, "telemetry", "VR telemetry (the headset's own server, no SteamVR here)");
            status_line(ui, s, "Telemetry");
            this.phone_address(ui, c, "telemetry");
            cfg_slider(ui, c, "telemetry_scale", "Scale (reach)", 0.5, 3.0);
            ui.label(`T-pose: ${s.telemetry_calibration}`);
            ui.row();
            if ui.button("Calibrate (press both triggers)") { tracking(#{ action: "telemetry_calibrate", now: false }); }
            if ui.button("Now") { tracking(#{ action: "telemetry_calibrate", now: true }); }
            if ui.button("Reset") { tracking(#{ action: "telemetry_reset" }); }
            ui.end();
            ui.small("Stand in a T-pose, arms out, palms down, looking ahead.");
            feel_sliders(ui, c, "telemetry_feel");
        },
        "Leap" => {
            input_on(ui, c, "leap", "Leap Motion (hands)");
            status_line(ui, s, "Leap");
            cfg_check(ui, c, "leap_on_head", "The Leap is on the headset, not on the desk");
            cfg_slider(ui, c, "leap_height", "Hands higher (+) or lower (-), metres", -0.6, 0.6);
            cfg_slider(ui, c, "leap_forward", "Hands forward (+) or back (-), metres", -0.6, 0.6);
            ui.row();
            if ui.button("Centre on my hands now") {
                let r = tracking(#{ action: "leap_centre" });
                this.note = if r.done { "Centred: hands where they are now rest in front of the avatar." } else { "No hands seen: hold them over the Leap, then click." };
            }
            if ui.button("Reset") {
                set_cfg(#{ leap_height: 0.0, leap_forward: 0.0 });
                this.note = "";
            }
            ui.end();
            if this.note != "" { ui.small(this.note); }
        },
        "Controller" => {
            input_on(ui, c, "controller", "Controller (the avatar holds one and presses its buttons)");
            status_line(ui, s, "Controller");
            ui.small("Set Arms to \"Controller (held)\" in General.");
            let backend = pick(ui, "Read through", c.controller.backend, [["auto", "Auto (XInput, else SDL)"], ["xinput", "XInput (Xbox controllers, Steam Input)"], ["sdl", "SDL (any controller: PlayStation, Switch Pro…)"]]);
            if backend != c.controller.backend { set_cfg(#{ controller: #{ backend: backend } }); }
            let pairs = [["", "The first one found"]];
            for d in this.lists.controllers { pairs.push([d[1], d[0]]); }
            let device = pick(ui, "Which one", c.controller.device, pairs);
            if device != c.controller.device { set_cfg(#{ controller: #{ device: device } }); }
            if type_of(avatar) == "map" {
                ui.separator();
                ui.strong(`On ${this.puppet}`);
                av_slider(ui, this.puppet, avatar, "controller_up", "Height (0 = the lowest), metres", 0.0, 0.4);
                av_slider(ui, this.puppet, avatar, "controller_forward", "Closer (-) or farther (+), metres", -0.2, 0.3);
                av_slider(ui, this.puppet, avatar, "controller_hand_up", "Hands lower (-) or higher (+), metres", -0.1, 0.15);
                av_slider(ui, this.puppet, avatar, "controller_hand_apart", "Hands closer (-) or wider (+), metres each", -0.05, 0.15);
                av_slider(ui, this.puppet, avatar, "controller_hand_roll", "Hands rotation (degrees)", -90.0, 90.0);
                av_slider(ui, this.puppet, avatar, "controller_hand_forward", "Hands back (-) or forward (+), metres", -0.1, 0.1);
                av_slider(ui, this.puppet, avatar, "controller_thumb_lift", "Thumbs lower (-) or higher (+)", -0.6, 0.6);
                this.model_path(ui, avatar, "controller_model", "Controller GLB (empty: the built-in one)");
            }
        },
        "Desk" => {
            input_on(ui, c, "desk", "Keyboard & mouse (the avatar at a desk, typing and mousing)");
            status_line(ui, s, "Desk");
            ui.small("Set Arms to \"Keyboard & mouse (at a desk)\" in General.");
            if type_of(avatar) == "map" {
                ui.separator();
                ui.strong(`On ${this.puppet}`);
                av_pick(ui, this.puppet, avatar, "desk_hands", "Hands", [["gaming", "One on the keys, the other on the mouse (gaming)"], ["switching", "Both typing; one goes to the mouse while it's used"], ["typing", "Both on the keys (typing only)"]]);
                av_pick(ui, this.puppet, avatar, "desk_keys", "What the keys show", [["area", "Area: WASD, Space, Shift, Ctrl exactly; others only tap (typing can't be read)"], ["exact", "Exact: every key (viewers could read your typing)"], ["motion", "Motion only: fingers tap, no keys shown"]]);
                av_check(ui, this.puppet, avatar, "desk_left_handed", "Left-handed (the mouse on the left)");
                av_slider(ui, this.puppet, avatar, "desk_up", "Desk lower (-) or higher (+), metres", -0.25, 0.3);
                av_slider(ui, this.puppet, avatar, "desk_forward", "Desk closer (-) or farther (+), metres", -0.2, 0.3);
                av_slider(ui, this.puppet, avatar, "desk_left_hand_up", "Left hand lower (-) or higher (+), metres", -0.1, 0.15);
                av_slider(ui, this.puppet, avatar, "desk_left_hand_forward", "Left hand back (-) or forward (+), metres", -0.1, 0.1);
                av_slider(ui, this.puppet, avatar, "desk_right_hand_up", "Right hand lower (-) or higher (+), metres", -0.1, 0.15);
                av_slider(ui, this.puppet, avatar, "desk_right_hand_forward", "Right hand back (-) or forward (+), metres", -0.1, 0.1);
                av_slider(ui, this.puppet, avatar, "desk_hand_tilt", "Hands tilted back (-) or forward (+), degrees", -45.0, 45.0);
                av_slider(ui, this.puppet, avatar, "desk_curl", "Fingers straighter (-) or more curled (+)", -0.6, 0.8);
                this.model_path(ui, avatar, "desk_model", "Keyboard & mouse GLB (empty: the built-in one)");
            }
        },
    }

    // The statuses change by themselves
    ui.repaint_after(0.5);
}

/// A phone's (or headset's) address: typed, saved with Enter or Apply
/// (called as this.phone_address: what's typed stays in the dock's edits)
fn phone_address(ui, c, key) {
    ui.row();
    let shown = if key in this.edits { this.edits[key] } else { c[key].ip };
    let typed = ui.text_box(shown, "its address: the app shows it");
    let entered = ui.entered();
    if ui.button("Apply") || entered {
        let m = #{};
        m[key] = #{ ip: typed };
        set_cfg(m);
        this.edits.remove(key);
    } else if typed != c[key].ip {
        this.edits[key] = typed;
    }
    ui.end();
}

/// A GLB's path on one Puppet: typed, saved with Enter or Apply
fn model_path(ui, a, key, label) {
    ui.label(label);
    ui.row();
    let shown = if key in this.edits { this.edits[key] } else { a[key] };
    let typed = ui.text_box(shown, "C:\\path\\to\\model.glb");
    let entered = ui.entered();
    if ui.button("Apply") || entered {
        avatar_set(this.puppet, key, typed);
        this.edits.remove(key);
    } else if typed != a[key] {
        this.edits[key] = typed;
    }
    ui.end();
}
