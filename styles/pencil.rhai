// Grease Pencil: a hand-drawn look, like Blender's Grease Pencil / Line Art.
// Flat fills with one hard shadow tone and paper grain; ink lines found in
// the picture (silhouettes, hair over the face, folds) whose width wanders
// and which are redrawn a few times a second ("boil", like animation on
// twos and threes).

const ID = "pencil";
const NAME = "Grease Pencil";

const SHADER = `
uniform float threshold;
uniform float shade_amount;
uniform float grain;
uniform float boil;

float hash(float2 p)
{
	return frac(sin(dot(p, float2(12.9898, 78.233))) * 43758.5453);
}

float4 style(Surface s, float4 c)
{
	// Flat fill: lit or shaded, nothing between
	float ndl = dot(s.normal, s.light) * 0.5 + 0.5;
	float lit = step(threshold, ndl);
	float3 shade = lerp(c.rgb, s.shade, shade_amount);
	float3 col = lerp(shade, c.rgb, lit);

	// Paper grain, redrawn with the lines
	float seed = floor(s.time * boil);
	float n = hash(floor(s.screen / 2.0) + seed * 13.17);
	col = col * (1.0 - grain * 0.25 * n);
	return float4(col, c.a);
}
`;

fn defaults() {
    #{
        threshold: 0.5,
        shade_amount: 0.8,
        ink: 3.0,
        line_color: 0xFF2A2830,
        jitter: 0.5,
        boil: 8.0,
        silhouettes: 0.02,
        folds: 0.5,
        grain: 0.3,
    }
}

fn properties() {
    [
        slider("threshold", "Light / shade split", 0.0, 1.0, 0.01),
        slider("shade_amount", "Shade colour", 0.0, 1.0, 0.01),
        slider("ink", "Line (pixels)", 0.0, 12.0, 0.5),
        color("line_color", "Line colour"),
        slider("jitter", "Line wobble", 0.0, 1.0, 0.01),
        slider("boil", "Redraws a second (0 = still)", 0.0, 24.0, 1.0),
        slider("silhouettes", "Edges: depth jump (lower = more)", 0.002, 0.2, 0.001),
        slider("folds", "Folds: surface turn (lower = more)", 0.05, 2.0, 0.01),
        slider("grain", "Paper grain", 0.0, 1.0, 0.01),
    ]
}

fn tick(dt) {
    let s = this.settings;
    #{
        threshold: s.threshold,
        shade_amount: s.shade_amount,
        grain: s.grain,
        boil: s.boil,
        lines: s.ink,
        lines_color: color_vec(s.line_color),
        lines_depth: s.silhouettes,
        lines_normal: s.folds,
        lines_jitter: s.jitter,
        lines_boil: s.boil,
    }
}
