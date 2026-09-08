use crate::{
    Error, Result,
    raster::{self, Artwork, Preview, RenderInput},
    settings::{self, Overrides, Settings},
};
use base64::Engine;
use image::ImageDecoder;
use resvg::usvg;
use serde::{Deserialize, Serialize};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
    pub path: Option<PathBuf>,
    pub test_pattern: bool,
    pub settings: Option<PathBuf>,
    pub defaults: bool,
    pub overrides: Overrides,
    pub snapshot: Option<Settings>,
}
fn invalid(detail: impl Into<String>) -> Error {
    Error::new("invalid_label", detail)
}
const ATTRS: &[&str] = &[
    "id",
    "width",
    "height",
    "viewBox",
    "preserveAspectRatio",
    "x",
    "y",
    "x1",
    "x2",
    "y1",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "dx",
    "dy",
    "d",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "transform",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "word-spacing",
    "visibility",
    "display",
    "style",
    "href",
    "space",
];
const STYLE: &[&str] = &[
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "word-spacing",
    "visibility",
    "display",
];
pub fn validate_svg(bytes: &[u8]) -> Result<(f64, f64)> {
    if bytes.len() > 1024 * 1024 {
        return Err(Error::localized("invalid_label", "err.svgLimit", &[]));
    }
    let xml = std::str::from_utf8(bytes)
        .map_err(|_| Error::localized("invalid_label", "err.svgUtf8", &[]))?;
    if xml.contains("<!") && (xml.contains("<!DOCTYPE") || xml.contains("<!ENTITY")) {
        return Err(Error::localized("invalid_label", "err.svgEntities", &[]));
    }
    let doc = roxmltree::Document::parse(xml).map_err(|e| invalid(e.to_string()))?;
    if doc.descendants().any(|node| node.is_pi()) {
        return Err(Error::localized("invalid_label", "err.svgInstruction", &[]));
    }
    let root = doc.root_element();
    if root.tag_name().name() != "svg"
        || root.tag_name().namespace() != Some("http://www.w3.org/2000/svg")
    {
        return Err(Error::localized("invalid_label", "err.svgRoot", &[]));
    }
    let view_box = if let Some(v) = root.attribute("viewBox") {
        let n: Vec<f64> = v
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| Error::localized("invalid_label", "err.viewBoxFormat", &[]))?;
        if n.len() != 4
            || n.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.)
            || n[2] <= 0.
            || n[3] <= 0.
        {
            return Err(Error::localized("invalid_label", "err.viewBoxRange", &[]));
        }
        Some((n[2], n[3]))
    } else {
        None
    };
    let axis = |name, fallback: Option<f64>| -> Result<(f64, Option<f64>)> {
        use svgtypes::LengthUnit;
        let Some(value) = root.attribute(name) else {
            return fallback
                .filter(|n| *n <= 100_000.)
                .map(|n| (n, None))
                .ok_or_else(|| Error::localized("invalid_label", "err.svgDimensions", &[]));
        };
        let length = value
            .parse::<svgtypes::Length>()
            .map_err(|e| invalid(e.to_string()))?;
        let factor = match length.unit {
            LengthUnit::None | LengthUnit::Px => 1.,
            LengthUnit::In => 203.2,
            LengthUnit::Cm => 80.,
            LengthUnit::Mm => 8.,
            LengthUnit::Pt => 203.2 / 72.,
            LengthUnit::Pc => 203.2 / 6.,
            _ => return Err(Error::localized("invalid_label", "err.svgUnits", &[])),
        };
        let pixels = length.number * factor;
        if !pixels.is_finite() || pixels <= 0. || pixels > 100_000. {
            return Err(Error::localized(
                "invalid_label",
                "err.svgDimensionRange",
                &[],
            ));
        }
        Ok((
            pixels,
            (length.unit == LengthUnit::Mm).then_some(length.number),
        ))
    };
    let width = axis("width", view_box.map(|v| v.0))?;
    let height = axis("height", view_box.map(|v| v.1))?;
    let size = match (width.1, height.1) {
        (Some(w), Some(h)) if (20. ..=50.).contains(&w) && (10. ..=100.).contains(&h) => (w, h),
        _ => (50., 30.),
    };
    let mut count = 0;
    let mut paths = 0;
    let mut image_bytes = 0;
    let mut image_pixels = 0u64;
    for node in root.descendants().filter(|n| n.is_element()) {
        count += 1;
        if count > 4096 || node.ancestors().count() > 33 {
            return Err(Error::localized("invalid_label", "err.svgDepth", &[]));
        }
        let tag = node.tag_name();
        if tag.namespace() != Some("http://www.w3.org/2000/svg")
            || ![
                "svg", "g", "text", "tspan", "path", "rect", "circle", "line", "image",
            ]
            .contains(&tag.name())
            || (tag.name() == "svg" && node != root)
        {
            return Err(Error::localized(
                "invalid_label",
                "err.svgElement",
                &[("name", tag.name().into())],
            ));
        }
        for attr in node.attributes() {
            let name = attr.name();
            let value = attr.value();
            if !ATTRS.contains(&name)
                || attr.namespace().is_some_and(|ns| {
                    !((name == "href" && ns == "http://www.w3.org/1999/xlink")
                        || (name == "space" && ns == "http://www.w3.org/XML/1998/namespace"))
                })
            {
                return Err(Error::localized(
                    "invalid_label",
                    "err.svgAttribute",
                    &[("name", name.to_string())],
                ));
            }
            if name == "href" {
                if tag.name() != "image" {
                    return Err(Error::localized("invalid_label", "err.svgHref", &[]));
                }
                let (mime, data) = value
                    .split_once(',')
                    .ok_or_else(|| Error::localized("invalid_label", "err.imageUri", &[]))?;
                let format = match mime {
                    "data:image/png;base64" => image::ImageFormat::Png,
                    "data:image/jpeg;base64" => image::ImageFormat::Jpeg,
                    _ => return Err(Error::localized("invalid_label", "err.imageMime", &[])),
                };
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| Error::localized("invalid_label", "err.imageBase64", &[]))?;
                image_bytes += decoded.len();
                if image_bytes > 512 * 1024 {
                    return Err(Error::localized("invalid_label", "err.inlineBytes", &[]));
                }
                let (w, h) = image::ImageReader::with_format(Cursor::new(&decoded), format)
                    .into_dimensions()
                    .map_err(|e| invalid(e.to_string()))?;
                image_pixels += u64::from(w) * u64::from(h);
                if w == 0 || h == 0 || image_pixels > 4_000_000 {
                    return Err(Error::localized("invalid_label", "err.inlinePixels", &[]));
                }
                let mut reader = image::ImageReader::with_format(Cursor::new(&decoded), format);
                let mut limits = image::Limits::default();
                limits.max_alloc = Some(32 * 1024 * 1024);
                reader.limits(limits);
                reader.decode().map_err(|e| invalid(e.to_string()))?;
            } else {
                if value.len() > if name == "d" { 128 * 1024 } else { 4096 } {
                    return Err(Error::localized(
                        "invalid_label",
                        "err.attributeLength",
                        &[],
                    ));
                }
                let lower = value.to_ascii_lowercase();
                if lower.contains("url")
                    || lower.contains('@')
                    || lower.contains('\\')
                    || lower.contains("http:")
                    || lower.contains("https:")
                    || lower.contains("file:")
                {
                    return Err(Error::localized(
                        "invalid_label",
                        "err.externalResource",
                        &[],
                    ));
                }
                if name == "d" {
                    paths += value.len();
                    if paths > 128 * 1024 {
                        return Err(Error::localized("invalid_label", "err.pathLimit", &[]));
                    }
                    for segment in svgtypes::PathParser::from(value) {
                        segment.map_err(|e| {
                            Error::localized(
                                "invalid_label",
                                "err.pathFormat",
                                &[("e", e.to_string())],
                            )
                        })?;
                    }
                }
                if name == "transform" {
                    for transform in svgtypes::TransformListParser::from(value) {
                        transform.map_err(|e| {
                            Error::localized(
                                "invalid_label",
                                "err.transformFormat",
                                &[("e", e.to_string())],
                            )
                        })?;
                    }
                }
                if [
                    "x",
                    "y",
                    "x1",
                    "y1",
                    "x2",
                    "y2",
                    "cx",
                    "cy",
                    "r",
                    "rx",
                    "ry",
                    "width",
                    "height",
                    "stroke-width",
                    "font-size",
                ]
                .contains(&name)
                {
                    let length = value
                        .parse::<svgtypes::Length>()
                        .map_err(|e| invalid(format!("{name}: {e}")))?;
                    if !length.number.is_finite() || length.number.abs() > 1_000_000. {
                        return Err(Error::localized(
                            "invalid_label",
                            "err.attributeRange",
                            &[("name", name.to_string())],
                        ));
                    }
                }
                if name == "style" {
                    for part in value.split(';').filter(|p| !p.trim().is_empty()) {
                        let (key, val) = part.split_once(':').ok_or_else(|| {
                            Error::localized("invalid_label", "err.styleFormat", &[])
                        })?;
                        if !STYLE.contains(&key.trim()) || val.contains(['{', '}', '!']) {
                            return Err(Error::localized(
                                "invalid_label",
                                "err.presentationStyle",
                                &[],
                            ));
                        }
                    }
                }
                if name == "font-family"
                    && !["Nanum Gothic", "NanumGothic", "sans-serif"]
                        .contains(&value.trim_matches(['\'', '"']))
                {
                    return Err(Error::localized("invalid_label", "err.fontFamily", &[]));
                }
            }
        }
        if tag.name() == "image" && !node.attributes().any(|a| a.name() == "href") {
            return Err(Error::localized("invalid_label", "err.imageHref", &[]));
        }
    }
    Ok(size)
}
fn options() -> usvg::Options<'static> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(include_bytes!("../../assets/fonts/NanumGothic-Regular.ttf").to_vec());
        db.set_sans_serif_family("NanumGothic");
        db.set_serif_family("NanumGothic");
        Arc::new(db)
    });
    usvg::Options {
        dpi: 203.2,
        font_family: "NanumGothic".into(),
        fontdb: fonts.clone(),
        font_resolver: usvg::FontResolver {
            select_font: Box::new(|_, db| db.faces().next().map(|face| face.id)),
            select_fallback: Box::new(|_, _, _| None),
        },
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|mime, data, _| match mime {
                "image/png" => Some(usvg::ImageKind::PNG(data)),
                "image/jpeg" => Some(usvg::ImageKind::JPEG(data)),
                _ => None,
            }),
        },
        ..Default::default()
    }
}
pub fn pattern(width: i32, height: i32) -> Vec<u8> {
    let (w, h) = (width, height);
    let (right, bottom) = (w - 1, h - 1);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}mm\" height=\"{}mm\" viewBox=\"0 0 {w} {h}\"><rect x=\"0.5\" y=\"0.5\" width=\"{right}\" height=\"{bottom}\" fill=\"none\" stroke=\"black\" stroke-width=\"1\"/><path d=\"M{} 0V{h} M0 {}H{w}\" stroke=\"black\" stroke-width=\"1\"/><path d=\"M8 32V8H32L8 32\" fill=\"black\"/>",
        f64::from(w) / 8.,
        f64::from(h) / 8.,
        w / 2,
        h / 2
    );
    for x in (40..w).step_by(40) {
        svg.push_str(&format!(
            "<path d=\"M{x} 0V12 M{x} {}V{h}\" stroke=\"black\" stroke-width=\"1\"/>",
            h - 12
        ));
    }
    for y in (40..h).step_by(40) {
        svg.push_str(&format!(
            "<path d=\"M0 {y}H12 M{} {y}H{w}\" stroke=\"black\" stroke-width=\"1\"/>",
            w - 12
        ));
    }
    svg.push_str(&format!("<text x=\"{}\" y=\"{}\" font-size=\"16\">한글 M110</text><rect x=\"{}\" y=\"{}\" width=\"16\" height=\"16\"/></svg>",w/2+8,h/2-16,w-28,h-28));
    svg.into_bytes()
}
// GIF decoders report the logical canvas, but may allocate a separate first-frame buffer.
fn validate_gif_frame(bytes: &[u8], width: u32, height: u32) -> Result<()> {
    let bad = || Error::localized("invalid_label", "err.gifFrame", &[]);
    let header = bytes.get(..13).ok_or_else(bad)?;
    let table_len = |flags: u8| {
        if flags & 0x80 != 0 {
            3 * (2usize << (flags & 7))
        } else {
            0
        }
    };
    let mut cursor = 13 + table_len(header[10]);
    loop {
        match *bytes.get(cursor).ok_or_else(bad)? {
            0x21 => {
                bytes.get(cursor + 1).ok_or_else(bad)?;
                cursor += 2;
                loop {
                    let size = usize::from(*bytes.get(cursor).ok_or_else(bad)?);
                    cursor += 1;
                    bytes.get(cursor..cursor + size).ok_or_else(bad)?;
                    cursor += size;
                    if size == 0 {
                        break;
                    }
                }
            }
            0x2c => {
                let frame = bytes.get(cursor + 1..cursor + 10).ok_or_else(bad)?;
                let word = |at| u32::from(u16::from_le_bytes([frame[at], frame[at + 1]]));
                let (x, y, w, h) = (word(0), word(2), word(4), word(6));
                if w == 0
                    || h == 0
                    || x.checked_add(w).is_none_or(|n| n > width)
                    || y.checked_add(h).is_none_or(|n| n > height)
                {
                    return Err(bad());
                }
                cursor += 10 + table_len(frame[8]);
                bytes.get(cursor).ok_or_else(bad)?; // LZW minimum code size.
                cursor += 1;
                let mut has_data = false;
                loop {
                    let size = usize::from(*bytes.get(cursor).ok_or_else(bad)?);
                    cursor += 1;
                    bytes.get(cursor..cursor + size).ok_or_else(bad)?;
                    cursor += size;
                    if size == 0 {
                        return if has_data { Ok(()) } else { Err(bad()) };
                    }
                    has_data = true;
                }
            }
            _ => return Err(bad()),
        }
    }
}
fn webp_error() -> Error {
    Error::localized("invalid_label", "err.webpContainer", &[])
}
// Check real slice ranges, including padding, before image-webp sees any declared size.
fn webp_chunk<'a>(remaining: &mut &'a [u8]) -> Result<(&'a [u8], &'a [u8])> {
    let header = remaining.get(..8).ok_or_else(webp_error)?;
    let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let end = 8usize.checked_add(len).ok_or_else(webp_error)?;
    let padded = end.checked_add(len & 1).ok_or_else(webp_error)?;
    let payload = remaining.get(8..end).ok_or_else(webp_error)?;
    // image-webp 0.2.4's first-ANMF indexer can read ALPH payload as a FourCC.
    // A conforming info byte cannot forge a recognized metadata chunk there.
    if &header[..4] == b"ALPH" && payload.first().is_none_or(|info| info & 0xe2 != 0) {
        return Err(webp_error());
    }
    *remaining = remaining.get(padded..).ok_or_else(webp_error)?;
    Ok((&header[..4], payload))
}
fn webp_u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}
fn webp_size(size: (u32, u32)) -> Result<()> {
    if size.0 == 0
        || size.1 == 0
        || size.0 > 16384
        || size.1 > 16384
        || u64::from(size.0) * u64::from(size.1) > 16_000_000
    {
        return Err(webp_error());
    }
    Ok(())
}
fn webp_bitstream(tag: &[u8], payload: &[u8], expected: Option<(u32, u32)>) -> Result<()> {
    let size = match tag {
        b"VP8 " => {
            let h = payload.get(..10).ok_or_else(webp_error)?;
            if h[0] & 1 != 0 || h[3..6] != [0x9d, 1, 0x2a] {
                return Err(webp_error());
            }
            (
                u32::from(u16::from_le_bytes([h[6], h[7]]) & 0x3fff),
                u32::from(u16::from_le_bytes([h[8], h[9]]) & 0x3fff),
            )
        }
        b"VP8L" => {
            let h = payload.get(..5).ok_or_else(webp_error)?;
            let bits = u32::from_le_bytes(h[1..5].try_into().unwrap());
            if h[0] != 0x2f || bits >> 29 != 0 {
                return Err(webp_error());
            }
            ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        _ => return Err(webp_error()),
    };
    webp_size(size)?;
    if expected.is_some_and(|expected| expected != size) {
        return Err(webp_error());
    }
    Ok(())
}
fn validate_webp(bytes: &[u8]) -> Result<()> {
    let header = bytes.get(..12).ok_or_else(webp_error)?;
    let declared = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    if &header[..4] != b"RIFF"
        || &header[8..] != b"WEBP"
        || declared.checked_add(8) != Some(bytes.len())
    {
        return Err(webp_error());
    }
    let mut remaining = &bytes[12..];
    let (first, payload) = webp_chunk(&mut remaining)?;
    let extended = first == b"VP8X";
    let (canvas, animated) = if extended {
        if payload.len() != 10 {
            return Err(webp_error());
        }
        let size = (webp_u24(&payload[4..7]) + 1, webp_u24(&payload[7..10]) + 1);
        webp_size(size)?;
        (Some(size), payload[0] & 2 != 0)
    } else {
        webp_bitstream(first, payload, None)?;
        (None, false)
    };
    let mut seen = 0u16;
    let mut frames = 0;
    while !remaining.is_empty() {
        let (tag, payload) = webp_chunk(&mut remaining)?;
        let bit = match tag {
            b"VP8X" => return Err(webp_error()),
            b"VP8 " | b"VP8L" => 1,
            b"ALPH" => 2,
            b"ANIM" => 4,
            b"EXIF" => 8,
            b"ICCP" => 16,
            b"XMP " => 32,
            _ => 0,
        };
        if seen & bit != 0 {
            return Err(webp_error());
        }
        seen |= bit;
        match tag {
            b"VP8 " | b"VP8L" => {
                if !extended || animated || (tag == b"VP8L" && seen & 2 != 0) {
                    return Err(webp_error());
                }
                webp_bitstream(tag, payload, canvas)?;
            }
            b"ALPH" if !extended || animated || seen & 1 != 0 => return Err(webp_error()),
            b"ANIM" if !animated || payload.len() != 6 => return Err(webp_error()),
            b"ANMF" => {
                if !animated {
                    return Err(webp_error());
                }
                let h = payload.get(..16).ok_or_else(webp_error)?;
                let size = (webp_u24(&h[6..9]) + 1, webp_u24(&h[9..12]) + 1);
                let (w, h_canvas) = canvas.ok_or_else(webp_error)?;
                if webp_u24(&h[..3]) * 2 + size.0 > w || webp_u24(&h[3..6]) * 2 + size.1 > h_canvas
                {
                    return Err(webp_error());
                }
                webp_size(size)?;
                let mut frame = &payload[16..];
                let (mut tag, mut data) = webp_chunk(&mut frame)?;
                if tag == b"ALPH" {
                    (tag, data) = webp_chunk(&mut frame)?;
                    if tag != b"VP8 " {
                        return Err(webp_error());
                    }
                }
                webp_bitstream(tag, data, Some(size))?;
                while !frame.is_empty() {
                    let (tail, _) = webp_chunk(&mut frame)?;
                    if matches!(
                        tail,
                        b"RIFF"
                            | b"WEBP"
                            | b"VP8X"
                            | b"VP8 "
                            | b"VP8L"
                            | b"ALPH"
                            | b"ANIM"
                            | b"ANMF"
                            | b"EXIF"
                            | b"ICCP"
                            | b"XMP "
                    ) {
                        return Err(webp_error());
                    }
                }
                frames += 1;
            }
            _ => {}
        }
    }
    if extended
        && (if animated {
            seen & 4 == 0 || frames == 0
        } else {
            seen & 1 == 0
        })
    {
        return Err(webp_error());
    }
    Ok(())
}
// image's TIFF wrapper does not expose ExtraSamples. Only the first classic IFD is needed.
fn tiff_associated_alpha(bytes: &[u8]) -> Result<bool> {
    let bad = || Error::localized("invalid_label", "err.tiffSamples", &[]);
    let little = bytes.get(..4).ok_or_else(bad)? == b"II\x2a\0";
    if !little && bytes.get(..4) != Some(b"MM\0\x2a") {
        return Err(bad());
    }
    let word = |data: &[u8]| {
        let pair = [data[0], data[1]];
        if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    };
    let long = |data: &[u8]| {
        let quad = [data[0], data[1], data[2], data[3]];
        if little {
            u32::from_le_bytes(quad)
        } else {
            u32::from_be_bytes(quad)
        }
    };
    let start = long(bytes.get(4..8).ok_or_else(bad)?) as usize;
    let ifd = bytes.get(start..).ok_or_else(bad)?;
    let count = usize::from(word(ifd.get(..2).ok_or_else(bad)?));
    let end = 2 + count * 12;
    ifd.get(..end + 4).ok_or_else(bad)?;
    let mut alpha = None;
    for entry in ifd[2..end].chunks_exact(12) {
        if word(entry) == 338 {
            if alpha.is_some() || word(&entry[2..]) != 3 || long(&entry[4..]) != 1 {
                return Err(bad());
            }
            let value = word(&entry[8..]);
            if value > 2 {
                return Err(bad());
            }
            alpha = Some(value == 1);
        }
    }
    Ok(alpha.unwrap_or(false))
}
fn decode_artwork(source: &[u8]) -> Result<((f64, f64), Artwork)> {
    use image::ImageFormat;
    match image::guess_format(source) {
        Ok(
            format @ (ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Gif
            | ImageFormat::Tiff),
        ) => {
            if format == ImageFormat::WebP {
                validate_webp(source)?;
            }
            let associated_alpha = format == ImageFormat::Tiff && tiff_associated_alpha(source)?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(16384);
            limits.max_image_height = Some(16384);
            limits.max_alloc = Some(128 * 1024 * 1024);
            let mut reader = image::ImageReader::with_format(Cursor::new(source), format);
            reader.limits(limits.clone());
            let mut decoder = reader.into_decoder().map_err(|e| {
                Error::localized("invalid_label", "err.imageDecode", &[("e", e.to_string())])
            })?;
            let (w, h) = decoder.dimensions();
            if w == 0
                || h == 0
                || w > 16384
                || h > 16384
                || u64::from(w) * u64::from(h) > 16_000_000
            {
                return Err(Error::localized(
                    "invalid_label",
                    "err.imageDimensions",
                    &[],
                ));
            }
            if format == ImageFormat::Gif {
                validate_gif_frame(source, w, h)?;
            }
            limits.reserve(decoder.total_bytes()).map_err(|e| {
                Error::localized("invalid_label", "err.imageMemory", &[("e", e.to_string())])
            })?;
            decoder
                .set_limits(limits)
                .map_err(|e| invalid(e.to_string()))?;
            let orientation = decoder.orientation().map_err(|e| invalid(e.to_string()))?;
            let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(|e| {
                Error::localized("invalid_label", "err.imageDecode", &[("e", e.to_string())])
            })?;
            decoded.apply_orientation(orientation);
            let mut rgba = decoded.into_rgba8();
            if associated_alpha {
                for pixel in rgba.pixels_mut() {
                    let alpha = u32::from(pixel[3]);
                    for channel in &mut pixel.0[..3] {
                        *channel = (u32::from(*channel) * 255 + alpha / 2)
                            .checked_div(alpha)
                            .unwrap_or(0)
                            .min(255) as u8;
                    }
                }
            }
            Ok(((50., 30.), Artwork::Bitmap(rgba)))
        }
        _ => {
            let xml = std::str::from_utf8(source)
                .ok()
                .map(|s| s.trim_start_matches('\u{feff}').trim_start());
            if !xml.is_some_and(|s| s.starts_with('<')) {
                return Err(Error::localized(
                    "invalid_label",
                    "err.imageUnsupported",
                    &[],
                ));
            }
            let size = validate_svg(source)?;
            let tree =
                usvg::Tree::from_data(source, &options()).map_err(|e| invalid(e.to_string()))?;
            if tree.size().width() > 100_000. || tree.size().height() > 100_000. {
                return Err(Error::localized("invalid_label", "err.svgSizeLimit", &[]));
            }
            Ok((size, Artwork::Svg(Box::new(tree))))
        }
    }
}
pub fn load(request: &Request) -> Result<RenderInput> {
    if request.defaults && request.settings.is_some() {
        return Err(Error::localized(
            "invalid_settings",
            "err.settingsConflict",
            &[],
        ));
    }
    if request.test_pattern && (request.path.is_some() || request.settings.is_some()) {
        return Err(Error::localized(
            "invalid_settings",
            "err.patternFiles",
            &[],
        ));
    }
    let source = if request.test_pattern {
        b"openlabel-calibration-v1".to_vec()
    } else {
        settings::bounded_read(
            request
                .path
                .as_deref()
                .ok_or_else(|| Error::localized("invalid_label", "err.imagePath", &[]))?,
            16 * 1024 * 1024,
            "invalid_label",
        )?
    };
    let (size, artwork) = if request.test_pattern {
        ((40., 30.), None)
    } else {
        let (size, artwork) = decode_artwork(&source)?;
        (size, Some(artwork))
    };
    let hash = crate::sha256(&source);
    let settings_path = if request.defaults || request.test_pattern {
        None
    } else if let Some(path) = &request.settings {
        Some(path.clone())
    } else {
        let path = settings::sidecar(request.path.as_ref().unwrap());
        match std::fs::symlink_metadata(&path) {
            Ok(_) => Some(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(Error::new("invalid_settings", e.to_string())),
        }
    };
    let sidecar_bytes = settings_path
        .as_ref()
        .map(|p| settings::bounded_read(p, 64 * 1024, "invalid_settings"))
        .transpose()?;
    let mut settings = if let Some(bytes) = &sidecar_bytes {
        let mut sidecar = serde_json::from_slice::<Settings>(bytes)
            .map_err(|e| Error::new("invalid_settings", e.to_string()))?;
        sidecar.validate(&hash)?;
        sidecar
    } else {
        Settings::defaults(&hash, size.0, size.1)
    };
    if let Some(snapshot) = &request.snapshot {
        settings = snapshot.clone();
        if request.test_pattern {
            settings.source_sha256 = hash.clone();
        }
        settings.validate(&hash)?;
    }
    settings.apply(&request.overrides);
    settings.validate(&hash)?;
    let artwork = if request.test_pattern {
        if settings.layout.scale_percent != 100. {
            return Err(Error::localized(
                "invalid_settings",
                "err.patternScale",
                &[],
            ));
        }
        let g = raster::Geometry::new(&settings)?;
        let (width, height) = if matches!(settings.layout.rotation_deg, 90 | 270) {
            (g.content_height, g.content_width)
        } else {
            (g.content_width, g.content_height)
        };
        Artwork::Svg(Box::new(
            usvg::Tree::from_data(&pattern(width, height), &options())
                .map_err(|e| invalid(e.to_string()))?,
        ))
    } else {
        artwork.unwrap()
    };
    Ok(RenderInput {
        source,
        sidecar_bytes,
        settings_path,
        settings,
        artwork,
    })
}
pub fn preview(request: &Request) -> Result<Preview> {
    raster::render(load(request)?)
}
pub async fn preview_async(request: Request) -> Result<Preview> {
    preview_controlled(request, &crate::operation::Control::default()).await
}
pub async fn preview_controlled(
    request: Request,
    control: &crate::operation::Control,
) -> Result<Preview> {
    control.check()?;
    static RENDER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let _permit = RENDER
        .try_acquire()
        .map_err(|_| Error::localized("render_busy", "err.renderBusy", &[]))?;
    let executable = std::env::current_exe().map_err(|e| invalid(e.to_string()))?;
    render_process(
        request,
        &executable,
        std::time::Duration::from_secs(5),
        control,
    )
    .await
}
async fn render_process(
    request: Request,
    executable: &std::path::Path,
    deadline: std::time::Duration,
    control: &crate::operation::Control,
) -> Result<Preview> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    request.overrides.validate_finite()?;
    if let Some(snapshot) = &request.snapshot {
        let mut settings = snapshot.clone();
        settings.validate(&snapshot.source_sha256)?;
    }
    let serialized =
        serde_json::to_vec(&request).map_err(|e| Error::new("invalid_settings", e.to_string()))?;
    control.check()?;
    let mut child = tokio::process::Command::new(executable)
        .arg("--internal-render-worker")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| invalid(e.to_string()))?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut output = Vec::new();
    let result = control
        .operation(async {
            tokio::time::timeout(deadline, async {
                stdin.write_all(&serialized).await?;
                drop(stdin);
                stdout
                    .take(2 * 1024 * 1024 + 1)
                    .read_to_end(&mut output)
                    .await?;
                child.wait().await
            })
            .await
            .map_err(|_| Error::localized("render_timeout", "err.renderTimeout", &[]))?
            .map_err(|e| invalid(e.to_string()))
        })
        .await;
    match result {
        Ok(status) if status.success() && output.len() <= 2 * 1024 * 1024 => {}
        result => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(result
                .err()
                .unwrap_or_else(|| Error::localized("render_timeout", "err.renderStopped", &[])));
        }
    }
    control.check()?;
    let (mut preview, packed): (Result<Preview>, String) =
        serde_json::from_slice(&output).map_err(|e| invalid(e.to_string()))?;
    if let Ok(p) = &mut preview {
        p.packed = base64::engine::general_purpose::STANDARD
            .decode(packed)
            .map_err(|e| invalid(e.to_string()))?;
        p.png = base64::engine::general_purpose::STANDARD
            .decode(&p.png_base64)
            .map_err(|e| invalid(e.to_string()))?;
    }
    preview
}
/// Private executable mode: only bounded rendering; it never discovers or writes to a device.
pub fn render_worker() -> bool {
    if std::env::args_os().nth(1).as_deref()
        != Some(std::ffi::OsStr::new("--internal-render-worker"))
    {
        return false;
    }
    use std::io::Read;
    let mut input = Vec::new();
    let result = std::io::stdin()
        .take(65537)
        .read_to_end(&mut input)
        .map_err(|e| invalid(e.to_string()))
        .and_then(|_| {
            if input.len() > 65536 {
                Err(Error::localized(
                    "invalid_label",
                    "err.renderRequestLimit",
                    &[],
                ))
            } else {
                serde_json::from_slice::<Request>(&input).map_err(|e| invalid(e.to_string()))
            }
        })
        .and_then(|r| preview(&r));
    let packed = result
        .as_ref()
        .map(|p| base64::engine::general_purpose::STANDARD.encode(&p.packed))
        .unwrap_or_default();
    println!("{}", serde_json::to_string(&(result, packed)).unwrap());
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn webp_alpha_preflight_blocks_forged_metadata_without_decoding() {
        let mut bytes = b"RIFF\x5e\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
        bytes.extend([0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend(b"ANIM\x06\0\0\0");
        bytes.extend([0; 6]);
        bytes.extend(b"ANMF\x32\0\0\0");
        bytes.extend([0; 16]);
        bytes.extend(b"ALPH\x08\0\0\0EXIF\0\0\0\x40VP8 \x0a\0\0\0");
        bytes.extend([0x10, 0, 0, 0x9d, 1, 0x2a, 1, 0, 1, 0]);
        assert_eq!(bytes.len(), 102);
        // Never call a decoder: a regression must not allocate the forged 1GiB EXIF.
        let error = validate_webp(&bytes).unwrap_err();
        assert!(error.detail.starts_with("WebP preflight:"));
        assert!(webp_chunk(&mut &b"ALPH\0\0\0\0"[..]).is_err());
        for info in 0u8..=255 {
            let mut short = b"ALPH\x01\0\0\0".to_vec();
            short.extend([info, 0]);
            let valid = info >> 6 == 0 && (info >> 4) & 3 <= 1 && info & 3 <= 1;
            assert_eq!(
                webp_chunk(&mut short.as_slice()).is_ok(),
                valid,
                "info={info:#x}"
            );
        }
    }
    #[tokio::test]
    async fn render_deadline_kills_and_reaps_worker() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("worker");
        let pidfile = dir.path().join("pid");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nwhile :; do :; done\n",
                pidfile.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let before = std::time::Instant::now();
        let error = render_process(
            Request::default(),
            &executable,
            std::time::Duration::from_secs(1),
            &crate::operation::Control::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "render_timeout");
        assert!(before.elapsed() < std::time::Duration::from_secs(3));
        let pid = std::fs::read_to_string(pidfile)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "worker must actually be gone before timeout returns"
        );
    }
}
