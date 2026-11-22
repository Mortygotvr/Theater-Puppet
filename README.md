# Theater Puppet

An OBS Studio plugin, written in Rust, that draws your avatar as an OBS source: **🎭 Theater Puppet**.

The avatar is a GLB prepared by [Theater Rig](../theater-rig), the Blender add-on. Its names say what each part is (Mixamo bones, `spring_` chains, ARKit shape keys, `Color<N>` slots), and Puppet adds the behaviour.

## Status

First version: it draws the avatar at rest (the T-pose from Theater Rig), with its textures, cut-out and blended transparency, and soft light. Colour-slot materials show their `color_default`.

Coming: posing and spring bones, colour slots, face tracking (iFacialMocap), body and hands (MediaPipe, Leap Motion).

## Using it

Add a source, **🎭 Theater Puppet**, and pick the GLB. Its properties:

- **Width / Height:** the source's size in pixels.
- **Zoom:** 1 shows the whole avatar.
- **Centre (feet to head):** what the frame is centred on.
- **Turn:** the avatar's rotation, in degrees.

Its log is `%APPDATA%\obs-studio\logs\theater_puppet.log`.

## Building (Windows)

You need Rust and OBS Studio 30 or newer. No OBS SDK or import libraries: it finds libobs's functions when OBS loads it.

1. `cargo build --release`
2. Close OBS, then run `deploy.ps1`. It copies the plugin into `C:\ProgramData\obs-studio\plugins\theater-puppet` and `%APPDATA%\obs-studio\plugins\theater-puppet`.

`cargo test` checks the GLB loader; `THEATER_PUPPET_GLB=<file.glb> cargo test -- --nocapture` also loads that avatar and lists what it found.

## License

GPL-2.0-or-later (see [LICENSE](LICENSE)).
