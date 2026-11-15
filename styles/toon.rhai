// Toon: MToon's look. Lit and shaded tones (the shade is each material's own
// shade_color, kept by Theater Rig), a soft or hard edge between them,
// optional extra bands, rim light and an outline.

const ID = "toon";
const NAME = "Toon";

const SHADER = `
uniform float threshold;
uniform float softness;
uniform float bands;
uniform float shade_amount;
uniform float rim;
uniform float4 rim_color;

float4 style(Surface s, float4 c)
{
	// 0 (facing away from the light) .. 1 (facing it)
	float ndl = dot(s.normal, s.light) * 0.5 + 0.5;
	// Bands: 1 is the classic lit / shaded pair
	float n = max(bands, 1.0);
	float x = saturate((ndl - threshold) / max(1.0 - threshold, 0.001) * 0.5 + 0.5) * n;
	float lit = saturate((floor(x) + smoothstep(0.5 - softness, 0.5 + softness, frac(x))) / n);
	float3 shade = lerp(c.rgb, s.shade, shade_amount);
	float3 col = lerp(shade, c.rgb, lit);
	// Rim: the edges facing away from the camera
	float edge = pow(1.0 - saturate(dot(s.normal, s.view)), 4.0);
	col = col + rim_color.rgb * edge * rim;
	return float4(col, c.a);
}
`;

fn defaults() {
    #{
        threshold: 0.5,
        softness: 0.03,
        bands: 1,
        shade_amount: 1.0,
        rim: 0.0,
        rim_color: 0xFFFFFFFF,
        outline: 1.5,
        outline_color: 0xFF1E1A1A,
    }
}

fn properties() {
    [
        slider("threshold", "Light / shade split", 0.0, 1.0, 0.01),
        slider("softness", "Edge softness", 0.0, 0.5, 0.01),
        int_slider("bands", "Bands", 1, 6, 1),
        slider("shade_amount", "Shade colour", 0.0, 1.0, 0.01),
        slider("rim", "Rim light", 0.0, 1.0, 0.01),
        color("rim_color", "Rim colour"),
        slider("outline", "Outline (mm)", 0.0, 10.0, 0.1),
        color("outline_color", "Outline colour"),
    ]
}

fn tick(dt) {
    let s = this.settings;
    #{
        threshold: s.threshold,
        softness: s.softness,
        bands: s.bands,
        shade_amount: s.shade_amount,
        rim: s.rim,
        rim_color: color_vec(s.rim_color),
        outline_width: s.outline / 1000.0,
        outline_color: color_vec(s.outline_color),
    }
}
