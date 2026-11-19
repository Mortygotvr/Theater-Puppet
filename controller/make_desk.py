"""The built-in keyboard and mouse Puppet's avatar uses while its Arms are on
the Keyboard & mouse input: a compact keyboard (a 60% layout and arrows) and
a mouse on a pad, made in Blender and exported as desk.glb.

    blender --background --python controller/make_desk.py -- controller/desk.glb

It's also the template for your own GLB (the source's "Keyboard & mouse
model" setting). Puppet looks for these names (any missing is just left
still):

- keyboard: the keyboard's body (it tells Puppet this GLB is a desk);
- key_<name>: each key (its origin on its top, at the middle), named as
  `KEYS` below: key_w, key_space, key_lshift, key_left…; they sink while
  pressed, when the source shows keys;
- mouse, mouse_left, mouse_right: the mouse and its buttons (they move
  together with your mouse; the buttons sink while clicked);
- mousepad: stays where it is.

Axes, as the user sees it: +X the user's left, +Z up, -Y away from the user
(Blender's axes; the GLB is exported Y up). Metres, a real keyboard's size:
Puppet scales it with the avatar. The keyboard sits left of the middle and
the mouse right of it; Puppet mirrors them for a left-handed user.
"""

import sys

import bpy

out = sys.argv[sys.argv.index("--") + 1]
bpy.ops.wm.read_factory_settings(use_empty=True)

U = 0.019  # a key's pitch


def material(name, rgb, rough=0.6):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*rgb, 1.0)
    bsdf.inputs["Roughness"].default_value = rough
    return m


BODY = material("Desk_Body", (0.07, 0.07, 0.08))
KEY = material("Desk_Key", (0.16, 0.16, 0.18), 0.5)
MOD = material("Desk_Mod", (0.26, 0.26, 0.29), 0.5)
PAD = material("Desk_Pad", (0.03, 0.03, 0.035), 0.9)

root = bpy.data.objects.new("desk", None)
bpy.context.collection.objects.link(root)


def finish(obj, name, mat, pivot):
    """Named, coloured, its origin at `pivot`, under the root."""
    obj.name = name
    obj.data.name = name
    obj.data.materials.append(mat)
    delta = [obj.location[i] - pivot[i] for i in range(3)]
    for v in obj.data.vertices:
        v.co = [v.co[i] + delta[i] for i in range(3)]
    obj.location = pivot
    obj.parent = root
    return obj


def rounded_box(name, size, at, mat, bevel=0.002, pivot=None, smooth=False, segments=3):
    bpy.ops.mesh.primitive_cube_add(size=1, location=at)
    obj = bpy.context.active_object
    obj.scale = size
    bpy.ops.object.transform_apply(scale=True)
    mod = obj.modifiers.new("bevel", "BEVEL")
    mod.width = bevel
    mod.segments = segments
    bpy.ops.object.modifier_apply(modifier="bevel")
    if smooth:
        bpy.ops.object.shade_smooth()
    return finish(obj, name, mat, pivot or at)


# The rows, from the user's left (+X) to right, as (name, width in keys);
# None is a gap. Row 0 is the farthest from the user.
ROWS = [
    [("esc", 1)] + [(str(n), 1) for n in [1, 2, 3, 4, 5, 6, 7, 8, 9, 0]] + [("minus", 1), ("equals", 1), ("backspace", 2)],
    [("tab", 1.5)] + [(c, 1) for c in "qwertyuiop"] + [("lbracket", 1), ("rbracket", 1), ("backslash", 1.5)],
    [("caps", 1.75)] + [(c, 1) for c in "asdfghjkl"] + [("semicolon", 1), ("quote", 1), ("enter", 2.25)],
    [("lshift", 2.25)] + [(c, 1) for c in "zxcvbnm"] + [("comma", 1), ("period", 1), ("slash", 1), ("rshift", 2.75)],
    [("lctrl", 1.25), ("lwin", 1.25), ("lalt", 1.25), ("space", 6.25), ("ralt", 1.25), ("rctrl", 1.25)],
]
MODS = {"esc", "backspace", "tab", "backslash", "caps", "enter", "lshift", "rshift", "lctrl", "lwin", "lalt", "ralt", "rctrl"}

# The keyboard: 15 keys wide, the arrows to its right; its middle at
# KB_X (left of the user's middle: the mouse goes on the right)
WIDTH = 15 * U
KB_X, KB_Y = 0.07, 0.0
TOP = 0.022  # the keys' tops
left_edge = KB_X + (WIDTH + 3.5 * U) / 2  # +X is the user's left
rounded_box("keyboard", (WIDTH + 4.0 * U, 5 * U + 0.016, 0.016), (KB_X, KB_Y, 0.008), BODY, 0.004)
for r, row in enumerate(ROWS):
    y = KB_Y - (2 - r) * U  # row 0 farthest (-Y)
    x = left_edge - 0.25 * U
    for name, w in row:
        cx = x - w * U / 2
        rounded_box(f"key_{name}", (w * U - 0.003, U - 0.003, 0.008), (cx, y, TOP - 0.004), MOD if name in MODS else KEY, 0.0015, (cx, y, TOP), segments=1)
        x -= w * U
# The arrows, right of the shifts and ctrls
ax = left_edge - 15.25 * U - 0.5 * U - 1.5 * U
for name, dx, row in (("up", 0, 3), ("left", 1, 4), ("down", 0, 4), ("right", -1, 4)):
    cx, y = ax + dx * U, KB_Y - (2 - row) * U
    rounded_box(f"key_{name}", (U - 0.003, U - 0.003, 0.008), (cx, y, TOP - 0.004), KEY, 0.0015, (cx, y, TOP), segments=1)

# The mouse on its pad, right of the keyboard (-X)
MX, MY = -0.26, 0.005
rounded_box("mousepad", (0.24, 0.2, 0.003), (MX + 0.01, MY, 0.0015), PAD, 0.001)
rounded_box("mouse", (0.062, 0.11, 0.032), (MX, MY + 0.01, 0.019), BODY, 0.02, smooth=True)
for name, dx in (("mouse_left", 0.0155), ("mouse_right", -0.0155)):
    rounded_box(name, (0.028, 0.045, 0.006), (MX + dx, MY - 0.025, 0.036), MOD, 0.004, (MX + dx, MY - 0.025, 0.036), smooth=True)

bpy.ops.object.select_all(action="SELECT")
bpy.ops.export_scene.gltf(filepath=out, export_format="GLB", use_selection=True, export_yup=True, export_apply=True, export_extras=True)
print("made", out)
