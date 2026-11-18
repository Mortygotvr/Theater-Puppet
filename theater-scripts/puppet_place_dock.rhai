// Theater Puppet's place dock: turn each Puppet, move it, play its
// animations. A Theater Scene Rhai dock; it asks Puppet through its OBS
// procedures theater_puppet_place (src/place.rs) and
// theater_puppet_animation (src/animation.rs). The sliders and the loop are
// the Puppet source's own settings.
const ID = "theater_puppet_place";
const NAME = "🧍 Puppet Place";
const KIND = "dock";

fn ui(ui) {
    let s = call_proc("theater_puppet_place", #{ action: "get" });
    if type_of(s) != "map" || !("puppets" in s) {
        ui.label("Theater Puppet isn't loaded.");
        ui.repaint_after(2.0);
        return;
    }
    if s.puppets.len() == 0 {
        ui.label("No Puppet source yet: add a 🎭 Theater Puppet to a scene.");
        ui.repaint_after(2.0);
        return;
    }

    // Each Puppet's animations, by its name
    let anims = #{};
    let r = call_proc("theater_puppet_animation", #{ action: "get" });
    if type_of(r) == "map" && "puppets" in r {
        for a in r.puppets {
            anims[a.source] = a;
        }
    }

    for p in s.puppets {
        ui.section(p.source);
        let turn = ui.slider("Turn (°)", p.turn, -180.0, 180.0);
        let x = ui.slider("Left / right (m)", p.x, -3.0, 3.0);
        let y = ui.slider("Down / up (m)", p.y, -2.0, 2.0);
        let z = ui.slider("Back / forward (m)", p.z, -3.0, 3.0);
        if turn != p.turn || x != p.x || y != p.y || z != p.z {
            call_proc("theater_puppet_place", #{ action: "set", source: p.source, turn: turn, x: x, y: y, z: z });
        }
        ui.row();
        if ui.button("Face front") {
            call_proc("theater_puppet_place", #{ action: "set", source: p.source, turn: 0.0 });
        }
        if ui.button("Turn around") {
            call_proc("theater_puppet_place", #{ action: "set", source: p.source, turn: p.turn + 180.0 });
        }
        if ui.button("Centre") {
            call_proc("theater_puppet_place", #{ action: "set", source: p.source, x: 0.0, y: 0.0, z: 0.0 });
        }
        ui.end();

        // Its own animations, made in Blender
        let a = anims[p.source];
        if a != () {
            if a.clips.len() == 0 {
                ui.small("No animations in this avatar's GLB (add them in Blender).");
            } else {
                let none = "(none: idle motion)";
                let current = if a.looping == "" { none } else { a.looping };
                let options = [none];
                options += a.clips;
                let picked = ui.choice("Loop", current, options);
                if picked != current {
                    let clip = if picked == none { "" } else { picked };
                    call_proc("theater_puppet_animation", #{ action: "loop", source: p.source, clip: clip });
                }
                ui.row();
                for c in a.clips {
                    if ui.button(`▶ ${c}`) {
                        call_proc("theater_puppet_animation", #{ action: "play", source: p.source, clip: c });
                    }
                }
                if a.playing != "" && ui.button("■ Stop") {
                    call_proc("theater_puppet_animation", #{ action: "stop", source: p.source });
                }
                ui.end();
            }
        }
        ui.end();
    }

    // The properties window may change them too
    ui.repaint_after(0.5);
}
