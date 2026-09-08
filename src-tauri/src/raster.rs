use crate::{
    Error, Result,
    settings::{Settings, dots, input_hash},
};
use base64::Engine;
use resvg::{tiny_skia, usvg};
use serde::Serialize;
use std::path::PathBuf;

pub struct RenderInput {
    pub source: Vec<u8>,
    pub sidecar_bytes: Option<Vec<u8>>,
    pub settings_path: Option<PathBuf>,
    pub settings: Settings,
    pub artwork: Artwork,
}
pub enum Artwork {
    Svg(Box<usvg::Tree>),
    Bitmap(image::RgbaImage),
}

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Geometry {
    pub paper_x: i32,
    pub paper_width: i32,
    pub height: i32,
    pub head_width: i32,
    pub printable_x: i32,
    pub printable_width: i32,
    pub content_x: i32,
    pub content_y: i32,
    pub content_width: i32,
    pub content_height: i32,
}
impl Geometry {
    pub fn new(s: &Settings) -> Result<Self> {
        let w = dots(s.paper.width_mm);
        let h = dots(s.paper.height_mm);
        let m = dots(s.layout.margin_mm);
        let x = match s.layout.alignment.as_str() {
            "left" => 0,
            "right" => 384 - w,
            _ => (384 - w).div_euclid(2),
        };
        let cx = (x + m).max(0);
        let cw = (x + w - m).min(384) - cx;
        let ch = h - 2 * m;
        if cw <= 0 || ch <= 0 {
            return Err(Error::localized(
                "invalid_settings",
                "err.emptyContent",
                &[],
            ));
        }
        Ok(Self {
            paper_x: x,
            paper_width: w,
            height: h,
            head_width: 384,
            printable_x: x.max(0),
            printable_width: (x + w).min(384) - x.max(0),
            content_x: cx,
            content_y: m,
            content_width: cw,
            content_height: ch,
        })
    }
}
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Preview {
    pub sha256: String,
    pub input_sha256: String,
    pub source_sha256: String,
    pub settings: Settings,
    pub settings_path: Option<std::path::PathBuf>,
    pub geometry: Geometry,
    pub clipped_dot_count: usize,
    pub png_base64: String,
    #[serde(skip)]
    pub packed: Vec<u8>,
    #[serde(skip)]
    pub png: Vec<u8>,
}
pub fn png_from_bits(bits: &[u8], height: u32) -> Result<Vec<u8>> {
    if height == 0 || bits.len() != 48 * height as usize {
        return Err(Error::localized("invalid_label", "err.rasterRows", &[]));
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 384, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::One);
        let mut writer = encoder
            .write_header()
            .map_err(|e| Error::new("invalid_label", e.to_string()))?;
        let white: Vec<u8> = bits.iter().map(|b| !b).collect();
        writer
            .write_image_data(&white)
            .map_err(|e| Error::new("invalid_label", e.to_string()))?;
    }
    Ok(png)
}
pub fn render(loaded: RenderInput) -> Result<Preview> {
    let s = loaded.settings;
    let g = Geometry::new(&s)?;
    let (w, h) = if matches!(s.layout.rotation_deg, 90 | 270) {
        (g.content_height as usize, g.content_width as usize)
    } else {
        (g.content_width as usize, g.content_height as usize)
    };
    let mut pix = tiny_skia::Pixmap::new(w as u32, h as u32)
        .ok_or_else(|| Error::localized("invalid_label", "err.rasterSize", &[]))?;
    pix.fill(tiny_skia::Color::WHITE);
    match loaded.artwork {
        Artwork::Svg(tree) => {
            let tree_size = tree.size();
            let mut scale = (w as f32 / tree_size.width()).min(h as f32 / tree_size.height());
            if s.layout.scale_percent != 100. {
                scale *= (s.layout.scale_percent / 100.) as f32;
            }
            let transform = tiny_skia::Transform::from_row(
                scale,
                0.,
                0.,
                scale,
                (w as f32 - tree_size.width() * scale) / 2.,
                (h as f32 - tree_size.height() * scale) / 2.,
            );
            resvg::render(&tree, transform, &mut pix.as_mut());
        }
        Artwork::Bitmap(mut pixels) => {
            for pixel in pixels.pixels_mut() {
                let alpha = u32::from(pixel[3]);
                for channel in &mut pixel.0[..3] {
                    *channel =
                        ((u32::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
                }
                pixel[3] = 255;
            }
            let factor = (w as f64 / f64::from(pixels.width()))
                .min(h as f64 / f64::from(pixels.height()))
                * s.layout.scale_percent
                / 100.;
            let width = (f64::from(pixels.width()) * factor).round().max(1.) as u32;
            let height = (f64::from(pixels.height()) * factor).round().max(1.) as u32;
            let resized = image::imageops::resize(
                &pixels,
                width,
                height,
                image::imageops::FilterType::Triangle,
            );
            let x = (w as i32 - width as i32).div_euclid(2);
            let y = (h as i32 - height as i32).div_euclid(2);
            for (sx, sy, pixel) in resized.enumerate_pixels() {
                let (dx, dy) = (x + sx as i32, y + sy as i32);
                if dx >= 0 && dx < w as i32 && dy >= 0 && dy < h as i32 {
                    let index = (dy as usize * w + dx as usize) * 4;
                    pix.data_mut()[index..index + 4].copy_from_slice(&pixel.0);
                }
            }
        }
    }
    let mut gray: Vec<f32> = pix
        .pixels()
        .iter()
        .map(|p| {
            let v = (299 * u32::from(p.red())
                + 587 * u32::from(p.green())
                + 114 * u32::from(p.blue())
                + 500)
                / 1000;
            if v >= u32::from(s.raster.white_cutoff) {
                255.
            } else {
                v as f32
            }
        })
        .collect();
    let mut bits = vec![0u8; 48 * g.height as usize];
    let mut clipped = 0;
    for y in 0..h {
        for x in 0..w {
            let index = y * w + x;
            let black = gray[index] < f32::from(s.raster.threshold);
            if s.raster.mode == "floyd-steinberg" {
                let err = gray[index] - if black { 0. } else { 255. };
                for (dx, dy, weight) in [(1, 0, 7.), (-1, 1, 3.), (0, 1, 5.), (1, 1, 1.)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && nx < w as i32 && ny < h as i32 {
                        gray[ny as usize * w + nx as usize] += err * weight / 16.;
                    }
                }
            }
            if black {
                let mut tx = x as i32;
                let mut ty = y as i32;
                if s.layout.mirror {
                    tx = w as i32 - 1 - tx;
                }
                (tx, ty) = match s.layout.rotation_deg {
                    90 => (h as i32 - 1 - ty, tx),
                    180 => (w as i32 - 1 - tx, h as i32 - 1 - ty),
                    270 => (ty, w as i32 - 1 - tx),
                    _ => (tx, ty),
                };
                tx += g.content_x + dots(s.layout.offset_x_mm);
                ty += g.content_y + dots(s.layout.offset_y_mm);
                if tx < g.printable_x
                    || tx >= g.printable_x + g.printable_width
                    || ty < 0
                    || ty >= g.height
                {
                    clipped += 1;
                } else {
                    bits[ty as usize * 48 + tx as usize / 8] |= 0x80 >> (tx % 8);
                }
            }
        }
    }
    let mut normalized = serde_json::json!([
        1,
        g.head_width,
        g.height,
        g.paper_width,
        &s.layout.alignment,
        dots(s.layout.margin_mm),
        s.layout.rotation_deg,
        s.layout.mirror,
        dots(s.layout.offset_x_mm),
        dots(s.layout.offset_y_mm),
        &s.raster.mode,
        s.raster.threshold,
        s.raster.white_cutoff,
    ]);
    if s.layout.scale_percent != 100. {
        normalized
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(s.layout.scale_percent));
    }
    let normalized = serde_json::to_vec(&normalized).unwrap();
    let mut identity = b"openlabel-raster-v1\0".to_vec();
    identity.extend_from_slice(&(normalized.len() as u64).to_le_bytes());
    identity.extend(normalized);
    identity.extend(&bits);
    let png = png_from_bits(&bits, g.height as u32)?;
    Ok(Preview {
        sha256: crate::sha256(&identity),
        input_sha256: input_hash(&loaded.source, loaded.sidecar_bytes.as_deref()),
        source_sha256: s.source_sha256.clone(),
        settings: s,
        settings_path: loaded.settings_path,
        geometry: g,
        clipped_dot_count: clipped,
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        packed: bits,
        png,
    })
}
