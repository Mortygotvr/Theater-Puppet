// Watercolour (whole avatar): a filter on the finished picture, in Theater
// Scene's Rhai filter format, so it works in both. Washes that soften and
// bleed, pigment pooling darker where washes meet, edges that wobble like
// wet paint, granulation in the paper's grain, highlights opening to the
// paper, and, if wanted, a faint pencil sketch along the shape (Theater
// Puppet's gbuf; the silhouette elsewhere).

const ID = "theater_watercolour_filter";
const NAME = "Watercolour (whole avatar)";
const BLEND = "premultiplied";

const SHADER = `
uniform float4x4 ViewProj;
uniform texture2d image;
uniform texture2d gbuf;
uniform float has_gbuf;
uniform float2 texel;
uniform float wash;
uniform float wash_mix;
uniform float wobble;
uniform float wobble_scale;
uniform float pooling;
uniform float granulation;
uniform float paper;
uniform float saturation;
uniform float bleed;
uniform float pencil;
uniform float4 pencil_color;
uniform float boil;
uniform float time;

sampler_state linear_clamp {
	Filter   = Linear;
	AddressU = Clamp;
	AddressV = Clamp;
};

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

float hash(float2 p)
{
	return frac(sin(dot(p, float2(12.9898, 78.233))) * 43758.5453);
}

// Smooth noise, 0..1
float vnoise(float2 p)
{
	float2 i = floor(p);
	float2 f = frac(p);
	float2 u = f * f * (3.0 - 2.0 * f);
	float a = hash(i);
	float b = hash(i + float2(1.0, 0.0));
	float c = hash(i + float2(0.0, 1.0));
	float d = hash(i + float2(1.0, 1.0));
	return lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y);
}

float fbm(float2 p)
{
	return vnoise(p) * 0.5 + vnoise(p * 2.03 + 17.1) * 0.3 + vnoise(p * 4.11 + 41.7) * 0.2;
}

float lum(float3 c)
{
	return dot(c, float3(0.299, 0.587, 0.114));
}

float distance_of(float4 g)
{
	return g.a + step(g.a, 0.0) * 100000.0;
}

// A faint line between this pixel and the one at uv: the silhouette, or
// where the shape folds or passes in front of itself
float sketch_at(float2 uv, float a0, float4 g0)
{
	float e = abs(image.Sample(point_clamp, uv).a - a0);
	if (has_gbuf > 0.5) {
		float4 g = gbuf.Sample(point_clamp, uv);
		float d0 = distance_of(g0);
		float d1 = distance_of(g);
		float jump = abs(d1 - d0) / min(d0, d1);
		float both = step(0.0001, g0.a) * step(0.0001, g.a);
		float turn = 1.0 - dot(g0.rgb * 2.0 - 1.0, g.rgb * 2.0 - 1.0);
		e = max(e, max(smoothstep(0.02, 0.05, jump), smoothstep(0.5, 0.8, turn) * both));
	}
	return e;
}

float4 PSWatercolour(VertData v) : TARGET
{
	float2 px = v.uv / texel;
	float seed = floor(time * boil) * 7.13;

	// Wet edges: where it samples drifts a little, slowly across the picture
	float2 drift = float2(fbm(px * wobble_scale + seed), fbm(px * wobble_scale + 31.7 + seed)) - 0.5;
	float2 uv = (px + drift * wobble * 2.0) * texel;

	// The wash: the colour around, by how much paint is there (a disc of 12)
	float4 here = image.Sample(linear_clamp, uv);
	float3 sum = here.rgb * here.a;
	float amount = here.a;
	for (int i = 0; i < 12; i++) {
		float angle = float(i) * 2.39996;
		float r = wash * sqrt((float(i) + 0.5) / 12.0);
		float4 s = image.Sample(linear_clamp, uv + float2(cos(angle), sin(angle)) * r * texel);
		sum += s.rgb * s.a;
		amount += s.a;
	}
	float3 washed = sum / max(amount, 0.0001);
	float spread = amount / 13.0;
	float3 col = lerp(here.rgb, washed, wash_mix);

	// Pigment pools where one wash meets another, and along the silhouette
	float meet = smoothstep(0.04, 0.25, length(here.rgb - washed)) * here.a;
	float rim = saturate((here.a - spread) * 3.0);
	col = col * (1.0 - pooling * max(meet, rim) * 0.6);

	// Granulation: pigment settling in the paper's grain
	// (coarse and soft: pigment gathering in the paper's dips, not noise)
	float grain = fbm(px * 0.12 + 3.0) - 0.5;
	col = col * (1.0 - granulation * grain * 0.45);

	// Transparent paint: highlights open toward the paper, colours a touch softer
	float l = lum(col);
	col = lerp(col, float3(1.0, 0.99, 0.96), paper * smoothstep(0.55, 1.0, l));
	col = lerp(float3(l, l, l), col, saturation);

	// The blot's edge: soft, bleeding, uneven
	float edge_noise = fbm(px * 0.06 + 11.0) - 0.5;
	float a = saturate(lerp(here.a, smoothstep(0.15, 0.75, spread + edge_noise * 0.35), bleed));

	// Pencil underneath (faint, a little off the paint)
	float4 g0 = gbuf.Sample(point_clamp, uv);
	float2 o = texel * 0.8;
	float sk = max(max(sketch_at(uv + float2(o.x, 0.0), here.a, g0), sketch_at(uv - float2(o.x, 0.0), here.a, g0)),
	               max(sketch_at(uv + float2(0.0, o.y), here.a, g0), sketch_at(uv - float2(0.0, o.y), here.a, g0)));
	float lead = saturate(sk) * pencil * pencil_color.a * (0.6 + 0.4 * vnoise(px * 0.3));
	float outa = max(a, lead);
	return float4(saturate(col) * a * (1.0 - lead) + pencil_color.rgb * lead, outa);
}

technique Draw
{
	pass
	{
		vertex_shader = VSDefault(v);
		pixel_shader  = PSWatercolour(v);
	}
}
`;

fn defaults() {
    #{
        wash: 5.0,
        wash_mix: 0.65,
        wobble: 2.5,
        wobble_scale: 0.02,
        pooling: 0.6,
        granulation: 0.25,
        paper: 0.4,
        saturation: 0.9,
        bleed: 0.6,
        pencil: 0.35,
        pencil_color: 0xFF4A3F3A,
        boil: 0.0,
    }
}

fn properties() {
    [
        slider("wash", "Wash (pixels the colour spreads)", 0.0, 16.0, 0.5),
        slider("wash_mix", "Wash strength", 0.0, 1.0, 0.01),
        slider("pooling", "Pigment pooling (dark rims where washes meet)", 0.0, 1.0, 0.01),
        slider("wobble", "Wet edges (pixels)", 0.0, 8.0, 0.1),
        slider("wobble_scale", "Wet edges: size of the waves", 0.005, 0.1, 0.001),
        slider("granulation", "Granulation (paper grain)", 0.0, 1.0, 0.01),
        slider("paper", "Highlights open to the paper", 0.0, 1.0, 0.01),
        slider("saturation", "Saturation", 0.0, 1.5, 0.01),
        slider("bleed", "Soft, uneven outline", 0.0, 1.0, 0.01),
        slider("pencil", "Pencil sketch", 0.0, 1.0, 0.01),
        color("pencil_color", "Pencil colour"),
        slider("boil", "Repainted a second (0 = still)", 0.0, 12.0, 1.0),
    ]
}

fn tick(dt) {
    let s = this.settings;
    let c = s.pencil_color;
    #{
        texel: [1.0 / this.width, 1.0 / this.height],
        wash: s.wash,
        wash_mix: s.wash_mix,
        wobble: s.wobble,
        wobble_scale: s.wobble_scale,
        pooling: s.pooling,
        granulation: s.granulation,
        paper: s.paper,
        saturation: s.saturation,
        bleed: s.bleed,
        pencil: s.pencil,
        pencil_color: [((c >> 16) & 255) / 255.0, ((c >> 8) & 255) / 255.0, (c & 255) / 255.0, ((c >> 24) & 255) / 255.0],
        boil: s.boil,
        time: this.time,
    }
}
