"""The built-in controller Puppet's avatar holds: a simple gamepad of the
standard layout, made in Blender and exported as gamepad.glb.

    blender --background --python controller/make_gamepad.py -- controller/gamepad.glb

It's also the template for your own controller GLB (the source's
"Controller model" setting). Puppet looks for these names (any missing is
just left still):

- hand_left, hand_right: empties where the wrists go (the hands hold there);
- stick_left, stick_right: tilt with the sticks (their origin is the pivot);
- trigger_left, trigger_right: turn with the triggers (origin: the hinge);
- bumper_left, bumper_right, button_a, button_b, button_x, button_y,
  button_back, button_start: sink a little while pressed;
- dpad: tilts toward the direction pressed.

Axes, as the holder sees it: +X the holder's left, +Z up, -Y away from the
holder (Blender's axes; the GLB is exported Y up). Metres, a real
controller's size: Puppet scales it with the avatar.
"""

import math
import sys

import bpy

out = sys.argv[sys.argv.index("--") + 1]
bpy.ops.wm.read_factory_settings(use_empty=True)


def material(name, rgb, rough=0.6):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*rgb, 1.0)
    bsdf.inputs["Roughness"].default_value = rough
    return m


BODY = material("Gamepad_Body", (0.08, 0.08, 0.09))
GREY = material("Gamepad_Grey", (0.22, 0.22, 0.24))
COLOURS = {
    "a": material("Gamepad_A", (0.15, 0.75, 0.25), 0.3),
    "b": material("Gamepad_B", (0.85, 0.15, 0.15), 0.3),
    "x": material("Gamepad_X", (0.15, 0.4, 0.9), 0.3),
    "y": material("Gamepad_Y", (0.95, 0.8, 0.1), 0.3),
}

root = bpy.data.objects.new("gamepad", None)
bpy.context.collection.objects.link(root)


def finish(obj, name, mat, pivot):
    """Named, coloured, its origin at `pivot`, under the root."""
    obj.name = name
    obj.data.name = name
    obj.data.materials.append(mat)
    # Origin at the pivot: the mesh moved, the object put there
    delta = [obj.location[i] - pivot[i] for i in range(3)]
    for v in obj.data.vertices:
        v.co = [v.co[i] + delta[i] for i in range(3)]
    obj.location = pivot
    obj.parent = root
    bpy.ops.object.shade_smooth()
    return obj


def rounded_box(name, size, at, mat, bevel=0.008, pivot=None):
    bpy.ops.mesh.primitive_cube_add(size=1, location=at)
    obj = bpy.context.active_object
    obj.scale = size
    bpy.ops.object.transform_apply(scale=True)
    mod = obj.modifiers.new("bevel", "BEVEL")
    mod.width = bevel
    mod.segments = 4
    bpy.ops.object.modifier_apply(modifier="bevel")
    return finish(obj, name, mat, pivot or at)


def disc(name, radius, depth, at, mat, pivot=None):
    bpy.ops.mesh.primitive_cylinder_add(radius=radius, depth=depth, location=at, vertices=24)
    return finish(bpy.context.active_object, name, mat, pivot or at)


# The body: a flat bar, and the two grips sloping back and down to the holder
rounded_box("body", (0.115, 0.06, 0.026), (0, 0, 0), BODY, 0.012)
for side, x in (("left", 1), ("right", -1)):
    grip = rounded_box(f"grip_{side}", (0.034, 0.07, 0.03), (x * 0.05, 0.03, -0.012), BODY, 0.013)
    grip.rotation_euler = (math.radians(-18), 0, math.radians(x * 12))

# Top: left stick upper left, d-pad lower left, ABXY upper right, right stick lower right
# ("upper": away from the holder, -Y)
top = 0.013
for name, x, y in (("stick_left", 0.038, -0.008), ("stick_right", -0.018, 0.014)):
    pivot = (x, y, top)
    disc(name, 0.009, 0.012, (x, y, top + 0.006), GREY, pivot)
rounded_box("dpad", (0.022, 0.022, 0.004), (0.018, 0.014, top + 0.002), GREY, 0.002, (0.018, 0.014, top))
cx, cy = -0.038, -0.008
for key, dx, dy in (("a", 0, 0.011), ("b", -0.011, 0), ("x", 0.011, 0), ("y", 0, -0.011)):
    disc(f"button_{key}", 0.0045, 0.004, (cx + dx, cy + dy, top + 0.002), COLOURS[key], (cx + dx, cy + dy, top))
for name, x in (("button_back", 0.01), ("button_start", -0.01)):
    disc(name, 0.003, 0.003, (x, -0.012, top + 0.0015), GREY, (x, -0.012, top))

# The far edge: bumpers on top, triggers below, hinged at their front
for side, x in (("left", 1), ("right", -1)):
    rounded_box(f"bumper_{side}", (0.035, 0.008, 0.008), (x * 0.042, -0.033, 0.006), GREY, 0.003)
    rounded_box(f"trigger_{side}", (0.022, 0.018, 0.03), (x * 0.044, -0.028, -0.012), GREY, 0.004, (x * 0.044, -0.038, -0.002))

# Where the wrists go: behind the grips, a little low
for side, x in (("left", 1), ("right", -1)):
    hand = bpy.data.objects.new(f"hand_{side}", None)
    bpy.context.collection.objects.link(hand)
    hand.location = (x * 0.072, 0.075, -0.02)
    hand.parent = root

bpy.ops.object.select_all(action="SELECT")
bpy.ops.export_scene.gltf(filepath=out, export_format="GLB", use_selection=True, export_yup=True, export_apply=True, export_extras=True)
print("made", out)
