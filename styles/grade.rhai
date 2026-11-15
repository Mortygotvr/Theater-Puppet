// Grade: brightness, contrast, saturation and a tint, on whatever the styles
// before it made. Put it after Toon (or alone, over the plain colours).

const ID = "grade";
const NAME = "Grade";

const SHADER = `
uniform float brightness;
uniform float contrast;
uniform float saturation;
uniform float4 tint;
uniform float tint_amount;

float4 style(Surface s, float4 c)
{
	float3 col = c.rgb * brightness;
	col = (col - 0.5) * contrast + 0.5;
	float grey = dot(col, float3(0.299, 0.587, 0.114));
	col = lerp(float3(grey, grey, grey), col, saturation);
	col = lerp(col, col * tint.rgb, tint_amount);
	return float4(saturate(col), c.a);
}
`;

fn defaults() {
    #{ brightness: 1.0, contrast: 1.0, saturation: 1.0, tint: 0xFFFFC8A0, tint_amount: 0.0 }
}

fn properties() {
    [
        slider("brightness", "Brightness", 0.0, 2.0, 0.01),
        slider("contrast", "Contrast", 0.0, 2.0, 0.01),
        slider("saturation", "Saturation", 0.0, 2.0, 0.01),
        color("tint", "Tint"),
        slider("tint_amount", "Tint amount", 0.0, 1.0, 0.01),
    ]
}

fn tick(dt) {
    let s = this.settings;
    #{
        brightness: s.brightness,
        contrast: s.contrast,
        saturation: s.saturation,
        tint: color_vec(s.tint),
        tint_amount: s.tint_amount,
    }
}
