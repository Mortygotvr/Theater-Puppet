# To test

What's built but not yet tried live, newest first. Tick it (or tell Claude
what went wrong) and it comes off the list.

## Spring bones per GLB
- [ ] 🪢 Spring bones (the avatar's box) lists only the kinds the avatar has
      (no avatar springs: says so).
- [ ] An Also draw GLB with springs: its box has "🪢 Its spring bones" with
      its own kinds' sliders; they change that GLB's springs only.

## Inputs sidelined by General; MeowFace removed
- [ ] With General → Expressions on MediaPipe only, iFacialMocap's tab says
      "receiving, but General gives Expressions… to MediaPipe".
- [ ] MeowFace is gone: no tab, not in General's lists; tracking settings
      that gave a part to it load with that part on Auto (the rest kept).

## GLBs with all-zero shape key data
- [ ] Desktop\Armature.glb (Theater Rig export) draws (it showed nothing:
      Puppet refused it). Set that Puppet's Zoom back to about 1.

## GLB prefab redeem (Scene → Puppet → Scene)
- [ ] In Blender: an animated GLB, timeline markers named "contact" on the
      hit frames, exported with Theater Rig enabled.
- [ ] Scene Studio: a 🎭 Puppet redeem → Action "📦 Spawn a GLB (prefab)":
      pick the GLB (📂); the Animation list shows its animations; the line
      under it says its length and how many contact frames it has.
- [ ] Place it (right/left, up, forward, size, turn); Test: it appears in
      the Puppet, plays once, fades out (0.3 s), is gone.
- [ ] Contact section: Contact Audio (Audio & Screen Shake) plays at each
      contact frame; Screen Shake and Impact Reaction set "On Contact" fire
      at each one; "On Start" fires as it spawns.
- [ ] Save the redeem, reopen it: the Contact Audio is still there.
- [ ] Several at once (fire it quickly): each plays on its own.

## Also draw GLBs: their own animation
- [ ] A GLB with a shape key animation (Shape Key Flipbook, or any made in
      Blender) as an Also draw GLB: its box has "Animation (loops; this
      GLB's own)", listing its clips. Pick one: it loops on that GLB.
- [ ] A flipbook: one shape per frame, no blending, the loop seamless.
- [ ] The avatar's own animations start at once (they used to hold their
      first frame a moment when exported from frame 1).

## Input from another computer (🌐 Network)
- [ ] Configuration → 🌐 Network: turn it on; it shows this computer's
      address and a pairing code. Allow OBS through the firewall if asked.
- [ ] On the Linux computer: `sudo usermod -aG input $USER` (log out and
      in), then run `theater-puppet-sender` (from
      sender/target/x86_64-unknown-linux-musl/release/) with the address
      and code. It lists the keyboard, mouse and controller it found.
- [ ] Keyboard & mouse / Controller on in Puppet, Arms set to one of them:
      the avatar types, mouses and holds the controller as you do on the
      Linux computer. Unplug/replug the controller: found again.
- [ ] Which face button is which on your controller (X/Y can be swapped by
      some Linux drivers): tell Claude if the wrong finger moves.

## Configuration window
- [ ] Keyboard & mouse (and Controller, General) tabs: the Puppet list starts on
      the Puppet on screen (marked "on screen"), so sliders move the avatar
      you see, live. Picking another one in the list sticks.

## Sky
- [ ] Puppet properties → Sky (was Reflection) → "Shows as": Light (the
      default now) lights the avatar with the picture's colours, with no
      gloss or mirror look. Mirror is the old reflection. Strength now goes
      up to 3 (Light is softer than Mirror).

## Camera
- [ ] Zoom goes up to 50 and keeps zooming in close (it used to stop
      getting bigger past about 6–8 on wide avatars).
- [ ] Puppet properties → Turn: the camera goes around the avatar (0 front,
      180 behind). The avatar doesn't spin and its hair doesn't swing. The
      light goes around with the camera. Puppet Place filters still turn
      the body.
- [ ] Puppet properties → Camera: Perspective / Orthographic (no
      perspective). Switching keeps the avatar the same size.

## Materials as Blender has them, light, framing
- [ ] The big carved block: its carving (flowers, gears) shows now. Its normal
      map is a 16-bit PNG, which used to load flat.
- [ ] Varnish: smooth materials (roughness near 0, your pillar and block)
      show sharp reflections of a light box and a ceiling strip, even on flat
      faces, in Toon and Standard. More or less with Toon's "Gloss".
- [ ] Blender's Coat (Principled BSDF → Coat Weight / Roughness): set it
      above 0 on a material, export, and a clear varnish layer shows over it.
      Your pillar and block have Coat 0; their shine is their roughness.
- [ ] Your MortyAvatar: the head shows its face (emission), the pillar its
      wood grain and carving (normal maps), the plaque its text. (Checked by
      snapshot; look at it in your scene.)
- [ ] Style "Standard (as Blender)" (Puppet properties → Style): gloss from
      roughness / metalness like EEVEE; Sun strength, World light, Specular.
- [ ] Toon has gloss now too: Toon settings → "Gloss" (the materials' own
      shine; 0 none) and "Gloss edge" (soft as Blender .. hard as anime).
      Your plaque and varnished wood shine; VRoid avatars don't (rough).
- [ ] Puppet properties → Light: "Its own light", around / height, colour,
      strength (Standard and Toon follow its colour).
- [ ] Zoom goes down to 0.05; a big prop beside a small avatar fits in view.
- [ ] A Puppet with no Avatar file, only Also draw GLBs: they draw.

## VR: how much of your movement the avatar copies
- [ ] ⚙ Puppet Configuration → VR telemetry (and SteamVR): "How much of your
      movement the avatar copies": Head turn, Head position, Hands, Hips,
      Feet (1 all, 0 none: the idle pose), and Smoothing on top.
- [ ] Hips at 0.3: leaning and stepping barely move the avatar's hips.
- [ ] Smoothing up: calmer, a little late; only the parts VR drives.
- [ ] They change live (no input restart, no hiccup).

## Also draw: what each GLB copies
- [ ] Puppet properties → each Also draw box: Copy movement / Copy rotation
      (from the armature or a bone), Copy shape keys. All on: as before.
- [ ] A part that mustn't move: all three off → it stays still.
- [ ] Copy rotation from a bone (e.g. Head), movement off: the GLB turns
      with that bone; from the armature: bone by bone as before.
- [ ] A GLB with expression keys the main avatar doesn't have (ARKit names
      or Fcl_…): they follow iFacialMocap / blinks / the microphone.
- [ ] Remove a middle GLB: the ones after it keep their copy settings.

## Theater Rig: Make Cloth Bones
- [ ] Reinstall the Rig zip. On an avatar with a skirt (not pants: a ring
      around both legs tears): Edit Mode, select the skirt's faces (or pick
      its material), Chains 8, Bones each 4, Kind Skirt → Make Cloth Bones.
      A ring of chains from the hips; Spring Preview swings it.
- [ ] A cape: a row of chains from the chest.
- [ ] Weight paint after: the waist stays on the hips, no seam between chains.
- [ ] Chains / Bones each changed, Make again (same name): replaced, nothing
      else touched. The ✕ removes it.

## Theater Rig: Spring preview
- [ ] Reinstall the Rig zip. After Rename Springs: Spring preview → Start.
      Move the armature (G), rotate it, pose the head, or play an animation:
      the hair / skirt swings, and the orange capsules (colliders) push it out.
- [ ] "Sway the body": the hips turn back and forth by themselves.
- [ ] Change a chain's Stiffness / Gravity / Drag / Radius while it runs: it
      changes at once; Export, load in Puppet: it swings the same way there.
- [ ] Stop: every bone back as it was. Ctrl+Z or opening a file while it
      runs doesn't break anything.
- [ ] Springs panel → "Delete Unused Spring Chains": on Rosia it says none
      (every chain bends the hair; end bones kept). On a model with a chain
      weighted on nothing, it asks first, lists it, deletes it; Ctrl+Z
      brings it back.

## Scene: 🎬 Puppet animation with an impact reaction
- [ ] A throwable redeem aimed at the Puppet → 💥 IMPACT REACTIONS → tick
      "🎬 Puppet animation", pick "Knocked Out" (▾ lists the Puppets' clips),
      On Contact. Test: the pie hits, the avatar bounces AND plays it once.
- [ ] Same with On Start: it plays as the redeem fires.
- [ ] Aimed at something that isn't a Puppet (a webcam): it plays on every Puppet.
- [ ] A GLB prefab redeem with it On Contact: plays at the GLB's contact frame.

## Tiles and the normal map
- [ ] Amouri (face atlas 2x2 "Emotion", a normal map on the face): in OBS,
      the face's bumps (nose, eyelids, lips) sit where they should, on
      Normal and on each tile (shy, gloom, salty), not shrunk to a quarter.
      (The shader change only compiles in OBS: watch the log for an effect
      error when it loads.)

## A GLB prefab following the avatar
- [ ] A prefab GLB with an armature named like the avatar's (a hat or wings
      skinned to Head / Spine2…, exported with Blender's glTF export): spawn
      it while you move: it stays on you, moving with you, and plays its own
      animation over that. Untick "🦴 Follow the avatar's armature": it's
      placed by the sliders, still.
- [ ] A prefab with no armature (a pie): placed by the sliders, as before.

## The avatar's Default animation
- [ ] Rig (reinstall the zip): "Default animation" above Export GLB: pick an
      action (breathing, a tail…). Export GLB: open the GLB in Windows 3D
      Viewer: it plays that animation.
- [ ] Puppet with the Animation setting on "(the avatar's default…)": the
      avatar loops it. "(the idle motion only)": back to the idle motion.
      A redeem animation plays over it, then it comes back.
- [ ] Scene: export a redeem playing that default: not listed as missing.

## Scene: a redeem exported is one file
- [ ] Export (.red) a prefab redeem whose reaction plays an Animation folder
      clip, with sounds and a custom script. On another PC (or after renaming
      your Animation folder's file away): Import it. It works as is: the GLB,
      sounds, script and the animation (now in the Puppet's Animation folder;
      a Puppet with none gets Theater's "animations" folder). The status line
      says where the animation went.
- [ ] A redeem playing a clip in no Animation folder and not the avatar's
      (a file you removed): Export says that animation is missing.

## Animations (any avatar, not through the avatar's GLB)
- [ ] Puppet source → 🎬 Pose and animation → Animation folder: a folder with
      Mixamo DAE ("With Skin") and Rig's animation GLBs. Their clips show in
      the Animation list and Scene's animation pickers; they play (try two avatars).
- [ ] A DAE downloaded Without Skin: the OBS log says to download it With Skin.
- [ ] Drop a new file in the folder while OBS runs, then "🔄 Reload GLBs"
      (top of the source's properties): it's in the list.
- [ ] Export the avatar again to the same file (no animation now): pick
      that file again in the source's properties (or Reload GLBs): the new
      one shows, its old animations gone from the lists. Nothing is polled:
      without one of those, OBS keeps the one it had.
- [ ] Rig (reinstall the zip), box "Animations" at the top, no avatar needed:
      Import Mixamo (FBX or DAE): its own skeleton next to the avatar, named
      after the file, playing in the timeline. Change it (add a "contact"
      marker), select it, Export Animation → its GLB in the folder you pick.
- [ ] Export Animation with the avatar's skeleton selected (an action made on
      it): works too. Export GLB of the avatar: no animation in it.
- [ ] Convert Mixamo to Animation GLB: a .glb next to each file; the scene
      unchanged (frame rate too).

## Theater Rig: Pick from Selected (colour slot, tiles)
- [ ] Edit Mode on the body, L on the hair (or a few of its faces), Pick:
      the hair's texture, not the skin's. Faces of two textures: the one with
      most faces, the other named in the message.
- [ ] Object Mode on a body with several textures: Pick shows a list with
      thumbnails; clicking one sets it.

## ⚙ Puppet Configuration window
- [ ] 🎭 Theater menu: Puppet's part (its icon) has "⚙ Puppet Configuration…";
      no "Tracking…" and no "🎬 Puppet Animations" any more.
- [ ] "⚙ Puppet Configuration…" opens a window (title bar, taskbar button);
      its ✕ hides it, the menu opens it again.
- [ ] Tabs: General, iFacialMocap, MediaPipe, SteamVR, VR telemetry,
      Leap Motion, Controller, Keyboard & mouse. Each one's switches, address
      (Enter or Apply), sliders and buttons work, and its "Now:" status updates.
- [ ] General / Controller / Keyboard & mouse: the Puppet list (with two
      Puppet sources) edits the one picked.
- [ ] Puppet properties no longer show the controller / keyboard & mouse
      options or "Follow tracking" (they're in the window).
- [ ] Typing in an address box: the keys go to the window.

## Keyboard & mouse (the avatar at a desk)
- [ ] Tracking window: turn on "Keyboard & mouse", set **Arms** to
      "Keyboard & mouse (at a desk)". The desk comes in front of the
      avatar, left hand by WASD, right hand on the mouse (zoom out to see).
- [ ] W A S D, Space, Shift, Ctrl: the right fingers press and the keys sink.
      Other keys (Area, the default): only a tap of the hand on that side,
      no key sinks.
- [ ] Moving the mouse moves the avatar's mouse and hand (now through a
      mouse hook: also in a game that holds the cursor); clicks bend the
      index / middle finger. Your own mouse doesn't feel any slower.
- [ ] Each hand's own height and forward / back sliders; the tilt and
      finger curl sliders.
- [ ] Puppet properties → "Keyboard & mouse: hands": switching (both
      typing, the right hand goes to the mouse while you use it) and typing.
- [ ] "What the keys show": Exact (every key) and Motion only.
- [ ] Left-handed: the mouse on the left, the right hand on the arrows.
- [ ] Desk lower / higher and closer / farther sliders; your own desk GLB.
- [ ] Hands, wrist angles and finger bends look right (worked out, not seen).

## Theater Rig: colour slot kept
- [ ] Reinstall the zip. On a MToon VRM: Make Colour Slot *before*
      Detoonify, then Export: the GLB's texture is the grey one (it used to
      go back to the old one).

## One tile standard (sets)
- [ ] Theater Rig (reinstall the zip): Tile textures → **Convert VRM Tiles**
      on HavocFFT: the box lists the set `expressions` with its 8 tiles.
- [ ] Make Tiles with a **Set** name: the box lists that set; ✕ removes it.
- [ ] Export, load in Puppet: Control Panel log lists the tiles by set.
- [ ] Control Panel "Puppet: Tile Texture": a name; `next` with a set
      (goes through it in the sheet's order); `random` (never the same twice).
- [ ] Scene: the tile pictures are grouped by set, each with ▶ next and
      🎲 random buttons.
- [ ] An old grid redeem (material / columns / rows) still fires; the form
      says so, and picking a tile switches it.

## Tile pictures in Scene
- [ ] Scene Studio → a Puppet redeem → "🧩 Tile texture": under "Tile:", a
      picture of each tile (every material it changes, side by side: face +
      eyes); click one to pick it, the picked one is lit.
- [ ] "A cell of a grid": pick the material, columns, rows: a picture of
      each cell; click to pick it.
- [ ] A GLB from Make Tiles (Theater Rig) shows its tiles the same way.

## Theater Rig: Make Tiles
- [ ] Reinstall Theater Rig from `Documents\theater_rig-0.1.0.zip` (both computers).
- [ ] Panel → Tile textures: pick a sheet texture (Pick from Selected), set
      columns / rows, names (`Open, Closed, Star…`, across then down), Make
      Tiles. The report says which materials were "moved" (UVs on one cell)
      or "over the whole sheet" (they now show the default cell in Blender).
- [ ] Export, load in Puppet: the tiles show by name in the Control Panel /
      Scene tile list and switch the texture.
- [ ] A VRM with its own tile expressions plus tiles you made: both kept.

## Timers and a tile's shape key
- [ ] Reload `puppet.tcp` in the Control Panel (Modules) so the new fields show.
- [ ] Tile with "Shape key with it" (e.g. a blush key) and Timer 10: the tile
      and the key come on, and both go back after 10 s.
- [ ] The same tile fired again before the 10 s run out: the timer restarts,
      and at the end you're back to the tile you had *before the first one*.
- [ ] Timer on a colour: the slot goes back to its colour before (or its own).
- [ ] Timer on Show GLB / Show GLB of group: the GLBs go back as they were
      (with the fade you set).
- [ ] Timer on a shape key: it turns off. On an animation loop: the loop
      before comes back.
- [ ] The same in a Scene Puppet redeem (Timer slider, and "Shape key with
      it" on a tile).

## Reflection
- [ ] Puppet properties → Reflection: pick your 360 background, strength
      ~0.3, "360": the avatar picks up its colours, more at the edges.
- [ ] "360: turn" lines the reflection up with the room.
- [ ] A flat source (a game capture) with "Flat".
- [ ] A scene that contains the Puppet itself: no freeze (the Puppet is
      left out of its own reflection).
- [ ] Cost: OBS's GPU time with it on (it draws the source once more a frame).

## Microphone lipsync
- [ ] Your mic source → Filters → add "🎤 Puppet Lipsync" (after noise
      suppression). Talking opens the avatar's mouth; silence keeps it shut
      (raise the noise gate if keyboard / fan noise moves it).
- [ ] Vowels: A, I, U, E, O look different (the vowel detection is tuned on
      synthetic voices, not yours yet: say which vowels it mixes up).
- [ ] Intensity, smoothing.
- [ ] "Shape key for A…O": your own keys; empty uses Fcl_MTH_A… (or ARKit).
- [ ] With iFacialMocap on: Add / Replace the mouth / Only when no face.

## Controller (the avatar holds one)
- [ ] Tracking window: turn on "Controller", "read through" Auto, then set
      **Arms** to "Controller (held)". The avatar holds the built-in gamepad
      in front of its belly (zoom out the Puppet source to see the hands).
- [ ] Hands and fingers look right around it (palms, thumbs on top, index
      fingers on the triggers). Their angles were worked out, not seen: say
      what's off.
- [ ] Buttons: A/B/X/Y (right thumb reaches, the button sinks), triggers and
      bumpers (index fingers), sticks (they tilt, the thumbs ride), d-pad.
- [ ] Your controller through XInput, and through SDL (Steam's SDL3.dll).
      A PlayStation / Switch controller through SDL.
- [ ] While playing a game (OBS not in front): it still follows.
- [ ] Puppet properties → "Controller it holds": your own GLB (make it from
      `controller/make_gamepad.py`'s names: hand_left/right, stick_left/right…).
- [ ] Arms set back to something else: the controller fades out of the hands.

## iFacialMocap head position
- [ ] Tracking window: Head position → iFacialMocap. Lean left / right, up /
      down, toward / away from the phone: the avatar's head follows, the
      right way and by the right amount (if it's 100× too much or too
      little, or mirrored, tell Claude: the units and axes were read from a
      sample, not seen live).
- [ ] 🎯 Puppet Tracking dock → iFacialMocap → "Re-centre head": where you
      sit now becomes the rest.

## Add / remove GLBs
- [ ] Put 3 GLBs in one group: one shows at a time.
- [ ] "Remove" on a middle one: the ones after it move up, keeping their
      files, Show and group.
- [ ] A source that had Also draw 1–3 before keeps them.

## Fades
- [ ] Show / hide a GLB with Fade 0.5 s: it dissolves in / out (a fine
      grain, then gone), outlines and filters too.
- [ ] A group swap with a fade: one dissolves out while the other comes in.
- [ ] Animate shape key with "hide at the end" and a fade.
- [ ] A tile with Crossfade 0.3 s: the old tile melts into the new one;
      `off` melts back. Grid `next` crossfades each step.
- [ ] Does the dissolve grain look good enough on stream, or should it be a
      true transparency (more work)?

## Tile textures
- [ ] Theater Rig: export HavocFFT.vrm (or WendigoFFT / Wendussy) with
      Export GLB; the report says "8 tile texture expression(s) kept".
- [ ] Puppet with that GLB: Control Panel "Puppet: Tile Texture" with name
      `Gigashad` (or a Scene Puppet redeem, action "🧩 Tile texture"): the skin
      and eyes switch to that tile; `off` brings them back.
- [ ] A grid tile: material `Wolf3D_Eye`, 2 × 2, cell `next` cycles the eyes.
- [ ] Any grid size works (try 4 × 2 on `Wolf3D_Skin`).
- [ ] A GLB exported with an animation from Blender keeps it now (Theater
      Rig's export used to drop animations).
- [ ] On Linux: Blender → Preferences → Get Extensions → Install from Disk →
      `Documents\theater_rig-0.1.0.zip` (rebuilt with the tiles).

## GLB groups
- [ ] Puppet properties: under each "Also draw", **Show** and **Group**. Two
      GLBs with the same group name: ticking Show on one hides the other.
- [ ] Control Panel "Puppet: Show GLB of Group" and Scene's Puppet redeem
      "Show a GLB of a group": a name, `next` (cycles), `none` (hides the group).
- [ ] Shown / hidden GLBs come back the same after an OBS restart.

## Control Panel module and Scene's Puppet redeem
- [ ] Control Panel → Modules → Load Module…
      `C:\ProgramData\obs-studio\plugins\theater-puppet\data\control-panel\puppet.tcp`;
      the log lists your Puppet's GLBs, animations, colour slots.
- [ ] Scene Studio: a redeem of type "🎭 Puppet"; its lists fill in (Puppets,
      GLBs, shape keys); Test fires it.
- [ ] Shape key on: `Fcl_MTH_A` stays open until a second command turns it off.
- [ ] Shape key ping-pong: opens and closes until turned off.
- [ ] Animate shape key: up, down, ping-pong over the duration, then back to normal.
- [ ] Animate shape key with "show at start / hide at end" on an Also draw GLB.
- [ ] Show / hide / toggle a GLB.
- [ ] Set colour slot from Control Panel and from a redeem; `#{1}` with a
      viewer's colour; `reset`.
- [ ] Play animation once / loop / stop (needs a GLB with clips from Blender).

## Colours
- [ ] Puppet properties → Colours → "Recolour slot 0": the hair takes the
      colour; its shadow the same colour, darker (not greyer).
- [ ] Theater Rig's greyscale textures look right recoloured (shading kept).

## Also draw (extra GLBs)
- [ ] "Also draw 1" = `Documents\Puppet_test_box.glb`: a red box in the
      avatar's right hand, following it, under the same filters.
      (Delete the test file afterwards.)

## Animations (from Blender)
- [ ] A GLB with clips: the "Animation" setting loops one in place of the idle motion.
- [ ] A shape-key clip (a face animation) plays; the body's idle motion stays on.

## Place filter
- [ ] Add "🧍 Puppet Place" to the Puppet; its sliders show in Scene Studio's
      filter part; two poses with different turns ease from one to the other.

## Tracking
- [ ] VR telemetry fingers (UDP 7501): finger curls on the avatar.
- [ ] SteamVR (needs a headset SteamVR can stream to; the Frame + Arc B580 can't).
- [ ] Head turns, hand positions and finger curls go the right way (not yet
      checked by watching a person).
