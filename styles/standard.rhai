// Standard: the look of Blender's Principled BSDF (EEVEE), the basic shader
// a GLB is made with. Lambert diffuse (less of it the more metallic), GGX
// specular with Smith shadowing and Schlick's Fresnel (Blender's Specular
// 0.5 is 4% at the front), from roughness and metalness (the material's,
// its metallic-roughness texture's), and the world's light all around.
// Worked in linear colour, as Blender does. The normal map and the emission
// are the template's: every style has them.

const ID = "standard";
const NAME = "Standard (as Blender)";

const SHADER = `
uniform float sun;
uniform float world;
uniform float specular;

float3 to_linear(float3 c)
{
	return pow(max(c, 0.0), 2.2);
}

float3 to_srgb(float3 c)
{
	return pow(max(c, 0.0), 1.0 / 2.2);
}

float4 style(Surface s, float4 c)
{
	float3 n = s.normal;
	float3 l = s.light;
	float3 v = s.view;
	float3 h = normalize(l + v);
	float ndl = saturate(dot(n, l));
	float ndv = max(dot(n, v), 0.0001);
	float ndh = saturate(dot(n, h));
	float vdh = saturate(dot(v, h));
	float3 base = to_linear(c.rgb);
	float r = clamp(s.roughness, 0.03, 1.0);
	float a = r * r;
	float a2 = a * a;
	// GGX's spread, Smith's shadowing, Schlick's Fresnel
	float d = ndh * ndh * (a2 - 1.0) + 1.0;
	float D = a2 / (3.14159265 * d * d);
	float k = (r + 1.0) * (r + 1.0) / 8.0;
	float G = (ndl / (ndl * (1.0 - k) + k)) * (ndv / (ndv * (1.0 - k) + k));
	float3 f0 = lerp(float3(0.08, 0.08, 0.08) * specular, base, s.metallic);
	float3 F = f0 + (1.0 - f0) * pow(1.0 - vdh, 5.0);
	float3 spec = D * G * F / (4.0 * ndv * max(ndl, 0.0001) + 0.0001) * ndl;
	// The sun, and the world's light from all around (its reflection on metal and gloss)
	float3 diffuse = base * (1.0 - s.metallic);
	float3 around = f0 + (max(float3(1.0 - r, 1.0 - r, 1.0 - r), f0) - f0) * pow(1.0 - ndv, 5.0);
	float3 lit = (diffuse * ndl + spec) * sun * s.light_color + (diffuse + around * 0.5) * world;
	return float4(to_srgb(lit), c.a);
}
`;

fn defaults() {
    #{
        sun: 2.2,
        world: 0.45,
        specular: 0.5,
        outline: 0.0,
        outline_color: 0xFF1E1A1A,
    }
}

fn properties() {
    [
        info("Shades like Blender's Principled BSDF: the GLB's colour, metalness, roughness, normal map and emission."),
        slider("sun", "Sun strength", 0.0, 6.0, 0.05),
        slider("world", "World light (all around)", 0.0, 2.0, 0.01),
        slider("specular", "Specular (Blender's 0.5)", 0.0, 1.0, 0.01),
        slider("outline", "Outline (mm)", 0.0, 10.0, 0.1),
        color("outline_color", "Outline colour"),
    ]
}

fn tick(dt) {
    let s = this.settings;
    #{
        sun: s.sun,
        world: s.world,
        specular: s.specular,
        outline_width: s.outline / 1000.0,
        outline_color: color_vec(s.outline_color),
    }
}
