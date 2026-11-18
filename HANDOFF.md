# Theater Puppet: handoff

State of the work on 2026-11-16, for whoever continues it. Theater Puppet is
an OBS source (Rust, `theater-puppet.dll`) that draws a Theater Rig avatar
(GLB) and moves it from tracking inputs.

## The owner's rules

- **The look comes from filters on the whole rendered picture** (Theater
  Scene's Rhai filter format), not from per-model 3D shaders. Several GLBs
  will be layered and must share one look cheaply, and filters will be
  switched on the fly. The 3D "styles" stay in the code but are secondary;
  do not build new looks as shaders.
- **Read the code path before proposing a fix** for a bug. No speculative
  fixes.
- **OBS may be restarted at will** to deploy, without checking its status
  or asking first.
- Each Theater part (Control Panel, Theater, Reader) has an old standalone
  repo and a new OBS-plugin repo. Work on the plugin versions.
- Merge or push only when asked. Nothing here has been pushed.

## Where things are

| What | Where |
|---|---|
| This plugin | `C:\Users\death\repo\Theater-Puppet`, branch `main` |
| Blender add-on that makes the GLB | `C:\Users\death\repo\theater-rig` |
| Theater Scene (redeems, physics) | `C:\Users\death\repo\theater scene(rust)` |
| Test avatar | `C:\Users\death\OneDrive\Documents\Armature.glb` (VRoid shape keys, no ARKit names) |
| Installed plugin | `C:\ProgramData\obs-studio\plugins\theater-puppet` |
| Settings, models, MediaPipe's Python | `%APPDATA%\obs-studio\plugin_config\theater-puppet` |
| Log | `%APPDATA%\obs-studio\logs\theater_puppet.log` |

## Build, test, deploy

```powershell
cargo build --release
$env:THEATER_PUPPET_GLB = "C:\Users\death\OneDrive\Documents\Armature.glb"
cargo test --release --lib -- --test-threads=1     # the avatar tests need the variable

# Deploy: OBS must be closed. Its "confirm on exit" blocks a polite close,
# and a forced stop brings a "Crash Detected" dialog unless the marker goes.
Stop-Process -Name obs64 -Force
Remove-Item "$env:APPDATA\obs-studio\.sentinel\run_*" -Force
.\deploy.ps1
Start-Process "C:\Program Files\obs-studio\bin\64bit\obs64.exe" `
  -ArgumentList "--disable-shutdown-check" `
  -WorkingDirectory "C:\Program Files\obs-studio\bin\64bit"
```

To see what the avatar looks like without the screen: create an empty file
`snapshot.request` in the settings folder; `snapshot.png` and `parts.png`
appear there a moment later.

## How a frame is made

1. Idle pose (`motion.rs`): arms down from the T-pose, breathing, sway, blink.
2. Tracking (`tracking/`): each input writes face, body and hands into shared
   state, already in the avatar's space (metres, Y up, facing +Z, its left at +X).
3. "What tracks what" (`tracking/mod.rs`, `config.rs`): per part, the input
   the user picked, or on Auto whichever sent last. Each part fades and is smoothed.
4. IK (`ik.rs`): body follows the head, feet planted, two-bone arms and legs, fingers.
5. Spring bones (`springs.rs`): chains named `spring_<kind>_<n>_<i>`, capsules
   fitted to the skin mesh at load.
6. CPU skinning with shape keys (`pose.rs`).
7. Draw into a picture plus a hidden one of normals and depth (`render.rs`).
8. Rhai filters on that picture (`filters.rs`, `filters/*.rhai`).
9. Body-part map for Theater Scene (`parts.rs`).

## Tracking inputs

| Input | How | Tested live |
|---|---|---|
| iFacialMocap | UDP 49983 | Yes |
| MeowFace | VTube Studio phone protocol, asks phone:21412, listens on 50506 | **No** (no Android phone); only the packet reading is tested |
| Leap Motion | `LeapC.dll` from the Leap software | Yes |
| SteamVR | `openvr_api.dll` from SteamVR, background then overlay app, `IVRSystem_023`; fingers from `IVRInput_010` skeletons | **No**: the owner's Steam Frame can't stream to this PC (SteamVR's vrlink refuses Intel Arc GPUs, vendor 0x8086) |
| VR telemetry | The headset's own telemetry server, as the owner's Unity app AvatarStreaming reads it: poses UDP 7500 (roles 1 head … 7 chest), finger skeletons 7501; T-pose calibration and a reach scale in the dock | Yes (headset, controllers, calibration, scale); fingers not yet confirmed |
| MediaPipe | Python helper `mediapipe/puppet_mediapipe.py`, started by `tracking/mediapipe.rs` | Yes |

Not verified by anyone: whether head turns, hand positions and finger curls
go the right way. They were built from the documented axes and unit tests,
not from watching a person.

MediaPipe notes:
- It installs its own Python the first time (uv, Python 3.12, mediapipe,
  opencv) into the settings folder, about 400 MB. A `python` entry in
  `tracking.json` overrides it; that field is deliberately not in the window.
- The helper opens the camera with OpenCV, by the camera's position in OBS's
  device list. A camera in use by an OBS source may not open.
- A venv's `python.exe` is only a launcher: stopping the helper kills the
  whole process tree, and the helper exits when OBS's process is gone. Do not
  go back to watching stdin for that; on Windows it blocked the imports.

## Place and animations

- Each Puppet has Turn and Position (X/Y/Z, metres) settings. Procedure
  `theater_puppet_place` (`src/place.rs`) reads and sets them, through the
  source's settings.
- **Animations come only from Blender** (the owner's decision): clips baked
  on the avatar's own skeleton, exported in its GLB. Puppet plays them as
  they are (`src/animation.rs`): a looping one in place of the idle motion
  (the source's "Animation" setting) and one played once over it. Tracking
  still goes over both. **Do not build Mixamo/FBX import or retargeting in
  Puppet**; if retargeting help is ever wanted, it belongs in Theater Rig.
  Procedure `theater_puppet_animation`. Shape key clips play too (matched by
  the key's name, the stronger of clip and tracking wins); a face-only clip
  leaves the idle body motion on. With Blender's "NLA tracks" export, a
  clip is named after its NLA track, not its action.
- The "🧍 Puppet Place" dock (`theater-scripts/puppet_place_dock.rhai`) has
  both. A test with a real clip runs when `THEATER_PUPPET_ANIMATED_GLB` points
  to an export with a "Wave" clip on RightArm.

## OBS graphics lessons already paid for

- Matrices must be transposed before `gs_effect_set_matrix4`.
- OBS clears effect parameters when a technique ends: set every parameter
  before every draw, and zero the numeric ones first, or the draw is skipped.
- Texture coordinate arrays must be width 2 or 4. `line` is reserved in HLSL.
- A whole-number attribute is interpolated across a triangle: the body-part
  map uses one-hot channels instead.

## Not merged

- Theater Scene, branch `puppet-body-parts`: aim redeems at Puppet's body
  parts, report the part hit; Rhai docks' `call_proc`, and other plugins'
  `data\theater-scripts` loaded. Puppet's "🎯 Puppet Tracking" dock
  (`theater-scripts/puppet_tracking_dock.rhai`, procedure
  `theater_puppet_tracking` in `src/tracking/dock.rs`) needs both.
- Theater Control Panel, branch `fix/reader-moderation-dedup`: the
  double-trigger fix. It is installed, but not merged.

## Discussed, not started

- Switching filters on the fly: preload, crossfade, a `theater_puppet_set_look`
  proc, saved looks.
- Several GLBs in one picture under one filter chain.
- Colour slot pickers (`Color<N>` materials already carry the slot).
- Puppet reacting to hits (Scene already emits `theater_scene_impact`).
- "vrtounity": the owner's own tracking, wanted as another input for
  themselves. Never specified here: ask what it sends and how before
  building it. A new input is one file in `src/tracking/` that calls
  `set_face` / `set_body` / `set_hands`, plus an entry in `Source` and in
  each `Part::inputs()` it can drive.
- Moving MediaPipe's detection into Rust, which would also drop OpenCV and
  take frames from OBS's own camera source. Days of work.

## Known limits

- Only a spring chain's joints collide, not the stretch between them.
- MediaPipe's head position takes the face's first position as "at rest".
- SteamVR's forward and origin are set when it connects; toggling the input
  sets them again.
