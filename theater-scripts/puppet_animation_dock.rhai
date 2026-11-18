// Theater Puppet's animation dock: each Puppet's own animations (made in
// Blender, in its GLB): the one looping in place of the idle motion, and any
// played once. A Theater Scene Rhai dock; it asks Puppet through its OBS
// procedure theater_puppet_animation (src/animation.rs). The loop is the
// Puppet source's own setting. (Turn and position: the "🧍 Puppet Place"
// filter, in the Studio's filter part.)
const ID = "theater_puppet_animation";
const NAME = "🎬 Puppet Animations";
const KIND = "dock";

fn ui(ui) {
    let r = call_proc("theater_puppet_animation", #{ action: "get" });
    if type_of(r) != "map" || !("puppets" in r) {
        ui.label("Theater Puppet isn't loaded.");
        ui.repaint_after(2.0);
        return;
    }
    if r.puppets.len() == 0 {
        ui.label("No Puppet source yet: add a 🎭 Theater Puppet to a scene.");
        ui.repaint_after(2.0);
        return;
    }

    for a in r.puppets {
        ui.section(a.source);
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
                call_proc("theater_puppet_animation", #{ action: "loop", source: a.source, clip: clip });
            }
            ui.row();
            for c in a.clips {
                if ui.button(`▶ ${c}`) {
                    call_proc("theater_puppet_animation", #{ action: "play", source: a.source, clip: c });
                }
            }
            if a.playing != "" && ui.button("■ Stop") {
                call_proc("theater_puppet_animation", #{ action: "stop", source: a.source });
            }
            ui.end();
        }
        ui.end();
    }

    // What's playing changes by itself
    ui.repaint_after(0.5);
}
