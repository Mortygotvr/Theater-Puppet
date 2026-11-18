// Theater Puppet's place dock: turn each Puppet and move it.
// A Theater Scene Rhai dock; it asks Puppet through its OBS procedure
// theater_puppet_place (src/place.rs). The sliders are the Puppet source's
// own Turn and Position settings.
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
        ui.end();
    }

    // The properties window may change them too
    ui.repaint_after(0.5);
}
