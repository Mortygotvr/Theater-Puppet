// Toon: MToon's look. Lit and shaded tones (the shade is each material's own
// shade_color, kept by Theater Rig), a soft or hard edge between them,
// optional extra bands, rim light, gloss (the material's own: its roughness
// and metalness, a highlight like Blender's, soft or hard) and an outline.

const ID = "toon";
const NAME = "Toon";

const SHADER = `
uniform float threshold;
uniform float softness;
uniform float bands;
uniform float shade_amount;
uniform float rim;
uniform float4 rim_color;
uniform float gloss;
uniform float gloss_hard;

float4 style(Surface s, float4 c)
{
	// 0 (facing away from the light) .. 1 (facing it)
	float ndl = dot(s.normal, s.light) * 0.5 + 0.5;
	// Bands: 1 is the classic lit / shaded pair
	float n = max(bands, 1.0);
	float x = saturate((ndl - threshold) / max(1.0 - threshold, 0.001) * 0.5 + 0.5) * n;
	float lit = saturate((floor(x) + smoothstep(0.5 - softness, 0.5 + softness, frac(x))) / n);
	float3 shade = lerp(c.rgb, s.shade, shade_amount);
	// (The lit side in the light's colour: white unless the source has its own light)
	float3 col = lerp(shade, c.rgb * s.light_color, lit);
	// Rim: the edges facing away from the camera
	float edge = pow(1.0 - saturate(dot(s.normal, s.view)), 4.0);
	col = col + rim_color.rgb * edge * rim;
	// Gloss: the material's highlight (GGX, as Blender's), tinted by its
	// metalness; none on a fully rough one (Detoonify makes VRoid's rough)
	float3 h = normalize(s.light + s.view);
	float ndh = saturate(dot(s.normal, h));
	float r = clamp(s.roughness, 0.04, 1.0);
	float a2 = r * r * r * r;
	float dd = ndh * ndh * (a2 - 1.0) + 1.0;
	float spec = a2 / (3.14159265 * dd * dd) * saturate(dot(s.normal, s.light)) * 0.25;
	// Hard: a clean-edged highlight, as anime draws it
	spec = lerp(spec, smoothstep(0.35, 0.45, saturate(spec)) * 1.2, gloss_hard);
	float3 f0 = lerp(float3(0.04, 0.04, 0.04), c.rgb, s.metallic);
	float smooth = saturate((1.0 - r) / 0.6);
	// The sun's highlight (a Blender sun's strength), and the surroundings
	// reflected: a studio's, bright above and dark below, blurred by
	// roughness, more at grazing angles (Fresnel): the sheen glass, varnish
	// and metal show in Blender
	float3 rv = reflect(-s.view, s.normal);
	float sky = lerp(saturate(rv.y * 0.5 + 0.5), 0.5, r);
	sky = sky * sky * 1.6;
	float ndv = saturate(dot(s.normal, s.view));
	float3 fresnel = f0 + (max(float3(1.0 - r, 1.0 - r, 1.0 - r), f0) - f0) * pow(1.0 - ndv, 5.0);
	col = col + (f0 * spec * 3.0 * s.light_color + fresnel * sky) * gloss * smooth;
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
        gloss: 1.0,
        gloss_hard: 0.0,
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
        slider("gloss", "Gloss (the materials' own shine: 0 none)", 0.0, 3.0, 0.05),
        slider("gloss_hard", "Gloss edge (soft as Blender .. hard as anime)", 0.0, 1.0, 0.01),
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
        gloss: s.gloss,
        gloss_hard: s.gloss_hard,
        outline_width: s.outline / 1000.0,
        outline_color: color_vec(s.outline_color),
    }
}
