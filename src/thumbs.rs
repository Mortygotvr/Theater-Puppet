//! Tile thumbnails, for Theater Scene's Puppet redeem form: the part of a
//! material's texture a tile shows (its mesh's UVs, placed as the tile
//! places them), small, as a PNG in base64.

use crate::model::Model;

/// The texture `material` of `model` shows with its UVs placed by `st`
/// (scale x, y, offset x, y), at most `size` pixels across; None when the
/// material has no texture or no mesh uses it
pub fn thumb(model: &Model, material: usize, st: [f32; 4], size: u32) -> Option<Vec<u8>> {
    png(&picture(model, material, st, size)?)
}

/// Each (material, placement) of a tile, side by side (a tile moving the
/// face and the eyes shows both); None when none has a picture
pub fn thumbs(model: &Model, binds: &[(usize, [f32; 4])], size: u32) -> Option<Vec<u8>> {
    let parts: Vec<image::RgbaImage> = binds.iter().filter_map(|(m, st)| picture(model, *m, *st, size)).collect();
    if parts.is_empty() {
        return None;
    }
    let gap = 2;
    let w = parts.iter().map(|p| p.width()).sum::<u32>() + gap * (parts.len() as u32 - 1);
    let h = parts.iter().map(|p| p.height()).max().unwrap_or(1);
    let mut out = image::RgbaImage::new(w, h);
    let mut x = 0;
    for p in &parts {
        image::imageops::overlay(&mut out, p, x as i64, ((h - p.height()) / 2) as i64);
        x += p.width() + gap;
    }
    png(&out)
}

fn png(img: &image::RgbaImage) -> Option<Vec<u8>> {
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

fn picture(model: &Model, material: usize, st: [f32; 4], size: u32) -> Option<image::RgbaImage> {
    let image = model.images.get(model.materials.get(material)?.image?)?;
    let (lo, hi) = uv_bounds(model, material)?;
    let place = |uv: [f32; 2]| [uv[0] * st[0] + st[2], uv[1] * st[1] + st[3]];
    let (a, b) = (place(lo), place(hi));
    let (u0, u1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (v0, v1) = (a[1].min(b[1]), a[1].max(b[1]));
    // Its shape, in the texture's pixels
    let w_px = ((u1 - u0) * image.width as f32).max(1.0);
    let h_px = ((v1 - v0) * image.height as f32).max(1.0);
    let k = size as f32 / w_px.max(h_px);
    let (w, h) = (((w_px * k).round() as u32).clamp(1, size), ((h_px * k).round() as u32).clamp(1, size));
    let mut out = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            // The texture wraps (as the sampler does)
            let u = u0 + (x as f32 + 0.5) / w as f32 * (u1 - u0);
            let v = v0 + (y as f32 + 0.5) / h as f32 * (v1 - v0);
            let px = ((u.rem_euclid(1.0) * image.width as f32) as u32).min(image.width - 1);
            let py = ((v.rem_euclid(1.0) * image.height as f32) as u32).min(image.height - 1);
            let i = ((py * image.width + px) * 4) as usize;
            out.put_pixel(x, y, image::Rgba([image.rgba[i], image.rgba[i + 1], image.rgba[i + 2], image.rgba[i + 3]]));
        }
    }
    Some(out)
}

/// The UVs of the meshes using `material`: lowest and highest
fn uv_bounds(model: &Model, material: usize) -> Option<([f32; 2], [f32; 2])> {
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for p in model.primitives.iter().filter(|p| p.material == material) {
        for uv in &p.uvs {
            for k in 0..2 {
                lo[k] = lo[k].min(uv[k]);
                hi[k] = hi[k].max(uv[k]);
            }
        }
    }
    lo[0].is_finite().then_some((lo, hi))
}

/// Base64 (standard, padded)
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

/// Base64 back (tests)
#[cfg(test)]
pub fn base64_decode(s: &str) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let vals: Vec<u32> = s.bytes().filter(|&b| b != b'=').map(|b| T.iter().position(|&t| t == b).unwrap() as u32).collect();
    let mut out = Vec::new();
    for c in vals.chunks(4) {
        let n = c.iter().enumerate().fold(0, |n, (i, v)| n | v << (18 - 6 * i));
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..c.len() - 1]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_goes_both_ways() {
        for s in [&b"a"[..], b"ab", b"abc", b"\x89PNG\r\n"] {
            assert_eq!(base64_decode(&base64(s)), s);
        }
    }

    #[test]
    fn base64_is_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
