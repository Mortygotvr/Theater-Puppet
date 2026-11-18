# To test

What's built but not yet tried live, newest first. Tick it (or tell Claude
what went wrong) and it comes off the list.

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
