# To test

What's built but not yet tried live, newest first. Tick it (or tell Claude
what went wrong) and it comes off the list.

## iFacialMocap head position
- [ ] Tracking window: Head position → iFacialMocap. Lean left / right, up /
      down, toward / away from the phone: the avatar's head follows, the
      right way and by the right amount (if it's 100× too much or too
      little, or mirrored, tell Claude: the units and axes were read from a
      sample, not seen live).
- [ ] 🎯 Puppet Tracking dock → iFacialMocap → "Re-centre head": where you
      sit now becomes the rest.

## Add / remove GLBs
- [ ] Puppet properties: "➕ Add a GLB" adds an "Also draw" box (GLB, Show,
      Group, Remove); add 4+ and put 3 in one group: one shows at a time.
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
- [ ] "🎬 Puppet Animations" dock: ▶ plays one once, ■ stops it.
- [ ] A shape-key clip (a face animation) plays; the body's idle motion stays on.

## Place filter
- [ ] Add "🧍 Puppet Place" to the Puppet; its sliders show in Scene Studio's
      filter part; two poses with different turns ease from one to the other.

## Tracking
- [ ] VR telemetry fingers (UDP 7501): finger curls on the avatar.
- [ ] SteamVR (needs a headset SteamVR can stream to; the Frame + Arc B580 can't).
- [ ] MeowFace (needs an Android phone).
- [ ] Head turns, hand positions and finger curls go the right way (not yet
      checked by watching a person).
