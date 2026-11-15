// Toon (whole avatar): a filter on the finished picture, in Theater Scene's
// Rhai filter format, so it works in both. Light and shade in flat bands,
// the shadow a colour layer blended over what's under it as in Procreate
// (Multiply, Color Burn, Linear Burn, Subtract), and lines drawn around the
// whole avatar at once:
// its silhouette, where colours change, and, in Theater Puppet, where its
// shape folds or overlaps (gbuf: each pixel's normal and distance).

const ID = "theater_toon_filter";
const NAME = "Toon (whole avatar)";
const BLEND = "premultiplied";

const SHADER = `
uniform float4x4 ViewProj;
uniform texture2d image;
uniform texture2d gbuf;
uniform float has_gbuf;
uniform float2 texel;
uniform float bands;
uniform float threshold;
uniform float softness;
uniform float shadow_mode;
uniform float4 shadow_color;
uniform float shadow_opacity;
uniform float light_x;
uniform float light_y;
uniform float saturation;
uniform float line_width;
uniform float4 line_color;
uniform float colour_lines;
uniform float shape_lines;

sampler_state point_clamp {
	Filter   = Point;
	AddressU = Clamp;
	AddressV = Clamp;
};

struct VertData {
	float4 pos : POSITION;
	float2 uv  : TEXCOORD0;
};

VertData VSDefault(VertData v)
{
	VertData o;
	o.pos = mul(float4(v.pos.xyz, 1.0), ViewProj);
	o.uv = v.uv;
	return o;
}

float lum(float3 c)
{
	return dot(c, float3(0.299, 0.587, 0.114));
}

// Nothing there reads as very far
float distance_of(float4 g)
{
	return g.a + step(g.a, 0.0) * 100000.0;
}

// How much of a line between this pixel and the one at uv
float edge_at(float2 uv, float4 c0, float4 g0)
{
	float4 c = image.Sample(point_clamp, uv);
	// The silhouette
	float e = abs(c.a - c0.a);
	// Where colours change (both there)
	e = max(e, smoothstep(0.08, 0.2, abs(lum(c.rgb) - lum(c0.rgb))) * colour_lines * c.a * c0.a);
	// Where the shape folds or one part passes in front of another
	if (has_gbuf > 0.5) {
		float4 g = gbuf.Sample(point_clamp, uv);
		float d0 = distance_of(g0);
		float d1 = distance_of(g);
		float jump = abs(d1 - d0) / min(d0, d1);
		float both = step(0.0001, g0.a) * step(0.0001, g.a);
		float turn = 1.0 - dot(g0.rgb * 2.0 - 1.0, g.rgb * 2.0 - 1.0);
		e = max(e, max(smoothstep(0.02, 0.04, jump), smoothstep(0.4, 0.6, turn) * both) * shape_lines);
	}
	return e;
}

float4 PSToon(VertData v) : TARGET
{
	float4 c0 = image.Sample(point_clamp, v.uv);
	float4 g0 = gbuf.Sample(point_clamp, v.uv);
	float2 o = texel * max(line_width, 0.0) * 0.5;
	float2 d = o * 0.7071;
	float e = max(max(edge_at(v.uv + float2(o.x, 0.0), c0, g0), edge_at(v.uv - float2(o.x, 0.0), c0, g0)),
	              max(edge_at(v.uv + float2(0.0, o.y), c0, g0), edge_at(v.uv - float2(0.0, o.y), c0, g0)));
	e = max(e, max(max(edge_at(v.uv + d, c0, g0), edge_at(v.uv - d, c0, g0)),
	               max(edge_at(v.uv + float2(d.x, -d.y), c0, g0), edge_at(v.uv - float2(d.x, -d.y), c0, g0))));

	// Light or shade: from the shape facing the light (Puppet), or how
	// bright the picture is (anywhere else)
	float t = saturate(lum(c0.rgb) * 1.4);
	if (has_gbuf > 0.5 && g0.a > 0.0) {
		float3 nrm = normalize(g0.rgb * 2.0 - 1.0);
		t = dot(nrm, normalize(float3(light_x, light_y, 1.0))) * 0.5 + 0.5;
	}
	// In bands: 1 is the classic lit / shaded pair
	float n = max(bands, 1.0);
	float x = saturate((t - threshold) / max(1.0 - threshold, 0.001) * 0.5 + 0.5) * n;
	float q = saturate((floor(x) + smoothstep(0.5 - softness, 0.5 + softness, frac(x))) / n);

	// The shadow is a layer, as in Procreate: its colour blended over the
	// colour under it (0 Multiply, 1 Color Burn, 2 Linear Burn, 3 Subtract),
	// at its opacity
	float3 base = saturate(c0.rgb);
	float3 sc = shadow_color.rgb;
	float3 layered = base * sc;
	if (shadow_mode > 2.5) {
		layered = base - sc;
	} else if (shadow_mode > 1.5) {
		layered = base + sc - 1.0;
	} else if (shadow_mode > 0.5) {
		layered = 1.0 - (1.0 - base) / max(sc, float3(0.001, 0.001, 0.001));
	}
	float3 shade = lerp(base, saturate(layered), shadow_opacity);
	float3 col = lerp(shade, c0.rgb, q);
	float grey = lum(col);
	col = saturate(lerp(float3(grey, grey, grey), col, saturation));

	// Premultiplied: the lines over it, and around it on the clear
	float ink = saturate(e) * line_color.a * step(0.01, line_width);
	float a = max(c0.a, ink);
	return float4(col * c0.a * (1.0 - ink) + line_color.rgb * ink, a);
}

technique Draw
{
	pass
	{
		vertex_shader = VSDefault(v);
		pixel_shader  = PSToon(v);
	}
}
`;

fn defaults() {
    #{
        bands: 1,
        threshold: 0.5,
        softness: 0.05,
        shadow_mode: 0,
        shadow_color: 0xFFA898C8,
        shadow_opacity: 1.0,
        light_x: -0.4,
        light_y: 0.6,
        saturation: 1.1,
        line_width: 2.0,
        line_color: 0xFF201C24,
        colour_lines: 0.5,
        shape_lines: 1.0,
    }
}

fn properties() {
    [
        int_slider("bands", "Bands (1 = light and shade)", 1, 6, 1),
        slider("threshold", "Light / shade split", 0.0, 1.0, 0.01),
        slider("softness", "Edge softness", 0.0, 0.5, 0.01),
        choice("shadow_mode", "Shadow blend mode", [["Multiply", 0], ["Color Burn", 1], ["Linear Burn", 2], ["Subtract", 3]]),
        color("shadow_color", "Shadow colour"),
        slider("shadow_opacity", "Shadow opacity", 0.0, 1.0, 0.01),
        slider("light_x", "Light from (left / right)", -1.5, 1.5, 0.05),
        slider("light_y", "Light from (below / above)", -1.5, 1.5, 0.05),
        slider("saturation", "Saturation", 0.0, 2.0, 0.01),
        slider("line_width", "Line (pixels)", 0.0, 10.0, 0.5),
        color("line_color", "Line colour"),
        slider("colour_lines", "Lines where colours change", 0.0, 1.0, 0.01),
        slider("shape_lines", "Lines where the shape folds (Puppet)", 0.0, 1.0, 0.01),
    ]
}

fn tick(dt) {
    let s = this.settings;
    let c = s.line_color;
    #{
        texel: [1.0 / this.width, 1.0 / this.height],
        bands: s.bands,
        threshold: s.threshold,
        softness: s.softness,
        shadow_mode: s.shadow_mode,
        shadow_color: [((s.shadow_color >> 16) & 255) / 255.0, ((s.shadow_color >> 8) & 255) / 255.0, (s.shadow_color & 255) / 255.0, 1.0],
        shadow_opacity: s.shadow_opacity,
        light_x: s.light_x,
        light_y: s.light_y,
        saturation: s.saturation,
        line_width: s.line_width,
        line_color: [((c >> 16) & 255) / 255.0, ((c >> 8) & 255) / 255.0, (c & 255) / 255.0, ((c >> 24) & 255) / 255.0],
        colour_lines: s.colour_lines,
        shape_lines: s.shape_lines,
    }
}
