use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use openlabel_core::{
    label::{self, Request},
    raster::Artwork,
    settings::{self, Overrides},
};
use std::{fs, io::Cursor, path::Path};

fn request(path: &Path) -> Request {
    Request {
        path: Some(path.to_path_buf()),
        ..Default::default()
    }
}
fn encode(format: ImageFormat) -> Vec<u8> {
    let pixels = RgbImage::from_fn(80, 40, |x, y| {
        if x < 20 && y < 30 {
            Rgb([0, 0, 0])
        } else {
            Rgb([255, 255, 255])
        }
    });
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(pixels)
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}
#[test]
fn clockwise_artwork_rotation_contains_svg_and_bitmap_before_physical_offsets() {
    let dir = tempfile::tempdir().unwrap();
    // Four unequal corner markers and an off-centre edge marker make direction observable.
    let marks = [
        (0, 0, 8, 12),
        (64, 0, 16, 4),
        (0, 32, 4, 8),
        (68, 28, 12, 12),
        (32, 0, 8, 4),
    ];
    let pixels = RgbaImage::from_fn(80, 40, |x, y| {
        let black = marks
            .iter()
            .any(|&(left, top, w, h)| x >= left && x < left + w && y >= top && y < top + h);
        if black {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    });
    let bitmap = dir.path().join("markers.png");
    pixels.save(&bitmap).unwrap();
    let vector = dir.path().join("markers.svg");
    let rectangles: String = marks
        .iter()
        .map(|(x, y, w, h)| format!("<rect x='{x}' y='{y}' width='{w}' height='{h}'/>"))
        .collect();
    fs::write(
        &vector,
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='80' height='40'>{rectangles}</svg>"
        ),
    )
    .unwrap();
    for path in [&vector, &bitmap] {
        for angle in [0, 90, 180, 270] {
            for mirror in [false, true] {
                for (scale, offset, floyd) in [
                    (100., 0., false),
                    (50., 0.5, false),
                    (150., 10., false),
                    (100., 0., true),
                ] {
                    let r = Request {
                        path: Some(path.clone()),
                        overrides: Overrides {
                            width_mm: Some(40.),
                            height_mm: Some(30.),
                            margin_mm: Some(0.),
                            rotation_deg: Some(angle),
                            mirror: Some(mirror),
                            scale_percent: Some(scale),
                            offset_x_mm: Some(offset),
                            offset_y_mm: Some(-offset),
                            raster_mode: Some(
                                if floyd {
                                    "floyd-steinberg"
                                } else {
                                    "threshold"
                                }
                                .into(),
                            ),
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let p = label::preview(&r).unwrap();
                    assert_eq!(
                        (
                            p.settings.paper.width_mm,
                            p.settings.paper.height_mm,
                            p.packed.len()
                        ),
                        (40., 30., 48 * 240)
                    );
                    let (w, h, fit) = if angle % 180 == 0 {
                        (320, 240, 4.)
                    } else {
                        (240, 320, 3.)
                    };
                    let factor = fit * scale / 100.;
                    let left = (f64::from(w) - 80. * factor) / 2.;
                    let top = (f64::from(h) - 40. * factor) / 2.;
                    let mut expected = image::GrayImage::from_fn(w, h, |x, y| {
                        let black = marks.iter().any(|&(mx, my, mw, mh)| {
                            f64::from(x) >= left + f64::from(mx) * factor
                                && f64::from(x) < left + f64::from(mx + mw) * factor
                                && f64::from(y) >= top + f64::from(my) * factor
                                && f64::from(y) < top + f64::from(my + mh) * factor
                        });
                        image::Luma([if black { 0 } else { 255 }])
                    });
                    if mirror {
                        expected = image::imageops::flip_horizontal(&expected);
                    }
                    expected = match angle {
                        90 => image::imageops::rotate90(&expected),
                        180 => image::imageops::rotate180(&expected),
                        270 => image::imageops::rotate270(&expected),
                        _ => expected,
                    };
                    let mut paper = image::GrayImage::from_pixel(384, 240, image::Luma([255]));
                    image::imageops::overlay(
                        &mut paper,
                        &expected,
                        32 + i64::from(settings::dots(offset)),
                        i64::from(settings::dots(-offset)),
                    );
                    // Physical paper clips x to 32..352 even when calibration shifts the artwork.
                    for (x, _, pixel) in paper.enumerate_pixels_mut() {
                        if !(32..352).contains(&x) {
                            pixel[0] = 255;
                        }
                    }
                    let actual = image::load_from_memory(&p.png).unwrap().to_luma8();
                    assert_eq!(actual.dimensions(), (384, 240));
                    if !floyd && path == &vector {
                        assert!(
                            actual == paper,
                            "{path:?}, {angle}, mirror={mirror}, scale={scale}, offset={offset}"
                        );
                    } else {
                        // Triangle filtering and dither can change edge pixels, not marker interiors.
                        for (x, y, pixel) in paper.enumerate_pixels() {
                            if x > 1
                                && x < 382
                                && y > 1
                                && y < 238
                                && [-2, -1, 0, 1, 2].iter().all(|dx| {
                                    [-2, -1, 0, 1, 2].iter().all(|dy| {
                                        paper.get_pixel(
                                            (x as i32 + dx) as u32,
                                            (y as i32 + dy) as u32,
                                        ) == pixel
                                    })
                                })
                            {
                                assert_eq!(actual.get_pixel(x, y), pixel);
                            }
                        }
                    }
                    if offset == 0. {
                        assert_eq!(p.clipped_dot_count, 0);
                    }
                    if !floyd && path == &vector {
                        assert_eq!(
                            p.clipped_dot_count,
                            expected.pixels().filter(|p| p[0] == 0).count()
                                - paper.pixels().filter(|p| p[0] == 0).count()
                        );
                    }
                    for (i, pixel) in actual.pixels().enumerate() {
                        assert_eq!(pixel[0] == 0, p.packed[i / 8] & (0x80 >> (i % 8)) != 0);
                    }
                }
            }
        }
    }
}

#[test]
fn rotated_pattern_keeps_forty_dot_ticks_on_non_square_paper() {
    for (width, height) in [(40., 30.), (50., 80.)] {
        for angle in [0, 90, 180, 270] {
            for mirror in [false, true] {
                let p = label::preview(&Request {
                    test_pattern: true,
                    overrides: Overrides {
                        width_mm: Some(width),
                        height_mm: Some(height),
                        rotation_deg: Some(angle),
                        mirror: Some(mirror),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap();
                let g = &p.geometry;
                assert_eq!(
                    (g.paper_width, g.height, p.packed.len()),
                    (
                        settings::dots(width),
                        settings::dots(height),
                        48 * settings::dots(height) as usize
                    )
                );
                assert_eq!(p.clipped_dot_count, 0);
                let (w, h) = if angle % 180 == 0 {
                    (g.content_width, g.content_height)
                } else {
                    (g.content_height, g.content_width)
                };
                // An independent edge-only ruler oracle; rotate it with image's tested operations.
                let mut ruler =
                    image::GrayImage::from_pixel(w as u32, h as u32, image::Luma([255]));
                for x in (40..w - 20).step_by(40) {
                    ruler.put_pixel(x as u32, 4, image::Luma([0]));
                }
                for y in (40..h - 20).step_by(40) {
                    ruler.put_pixel(4, y as u32, image::Luma([0]));
                }
                if mirror {
                    ruler = image::imageops::flip_horizontal(&ruler);
                }
                ruler = match angle {
                    90 => image::imageops::rotate90(&ruler),
                    180 => image::imageops::rotate180(&ruler),
                    270 => image::imageops::rotate270(&ruler),
                    _ => ruler,
                };
                let actual = image::load_from_memory(&p.png).unwrap().to_luma8();
                for (x, y, pixel) in ruler.enumerate_pixels() {
                    if pixel[0] == 0 {
                        assert!(
                            [-1, 0, 1].iter().any(|dx| [-1, 0, 1].iter().any(|dy| actual
                                .get_pixel(
                                    (x as i32 + g.content_x + dx) as u32,
                                    (y as i32 + g.content_y + dy) as u32
                                )[0]
                                == 0)),
                            "{width}x{height} angle={angle} mirror={mirror} tick={x},{y}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn six_formats_use_original_bytes_and_bounded_exact_one_bit_preview() {
    let dir = tempfile::tempdir().unwrap();
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Tiff,
    ] {
        let bytes = encode(format);
        let path = dir.path().join("wrong.svg"); // Content, never the extension, selects the decoder.
        fs::write(&path, &bytes).unwrap();
        let mut r = request(&path);
        r.defaults = true;
        let p = label::preview(&r).unwrap();
        assert_eq!(p.source_sha256, openlabel_core::sha256(&bytes));
        assert_eq!(p.input_sha256, settings::input_hash(&bytes, None));
        assert_eq!(
            (p.settings.paper.width_mm, p.settings.paper.height_mm),
            (50., 30.)
        );
        assert_eq!(p.packed.len(), 48 * 240);
        assert_eq!(p.png[24], 1); // PNG IHDR depth.
        let decoded = image::load_from_memory(&p.png).unwrap().to_luma8();
        assert_eq!(decoded.dimensions(), (384, 240));
        for (i, pixel) in decoded.pixels().enumerate() {
            assert_eq!(pixel[0] == 0, p.packed[i / 8] & (0x80 >> (i % 8)) != 0);
        }
        assert!(p.packed.iter().any(|b| *b != 0));
        r.snapshot = Some(p.settings.clone());
        assert_eq!(label::preview(&r).unwrap().sha256, p.sha256);
        let mut side = p.settings;
        side.layout.scale_percent = 75.5;
        fs::write(settings::sidecar(&path), serde_json::to_vec(&side).unwrap()).unwrap();
        r.defaults = false;
        r.snapshot = None;
        assert_eq!(
            label::preview(&r).unwrap().settings.layout.scale_percent,
            75.5
        );
        side.paper.width_mm = 51.;
        fs::write(settings::sidecar(&path), serde_json::to_vec(&side).unwrap()).unwrap();
        r.overrides.width_mm = Some(40.);
        assert_eq!(label::preview(&r).unwrap_err().code, "invalid_settings");
    }
}
#[test]
fn source_geometry_is_separate_from_strict_paper_and_scale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("geometry.svg");
    for attrs in [
        "width='80mm' height='50mm'",
        "width='100mm' height='80mm'",
        "width='100mm' height='5mm'",
        "width='5mm' height='80mm'",
        "width='800px' height='400px'",
        "width='2in' height='1in'",
        "viewBox='0 0 800 400'",
    ] {
        fs::write(&path, format!("<svg xmlns='http://www.w3.org/2000/svg' {attrs}><rect width='50' height='20'/></svg>")).unwrap();
        let mut r = request(&path);
        let original = label::preview(&r).unwrap();
        assert_eq!(
            (
                original.settings.paper.width_mm,
                original.settings.paper.height_mm
            ),
            (50., 30.)
        );
        r.overrides.width_mm = Some(40.);
        r.overrides.height_mm = Some(30.);
        r.overrides.scale_percent = Some(50.);
        let changed = label::preview(&r).unwrap();
        assert_ne!(original.sha256, changed.sha256);
        assert_eq!(changed.settings.layout.scale_percent, 50.);
        r.overrides.width_mm = Some(51.);
        assert_eq!(label::preview(&r).unwrap_err().code, "invalid_settings");
    }
    for attrs in [
        "",
        "width='0' height='1'",
        "width='100001' height='1'",
        "width='NaN' height='1'",
        "viewBox='0 0 0 10'",
        "width='50%' height='20%'",
    ] {
        assert!(
            label::validate_svg(
                format!("<svg xmlns='http://www.w3.org/2000/svg' {attrs}/>").as_bytes()
            )
            .is_err()
        );
    }
}
#[test]
fn scale_preserves_legacy_goldens_and_changes_print_identity() {
    let mut r = Request {
        path: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/sample.svg")),
        defaults: true,
        ..Default::default()
    };
    let original = label::preview(&r).unwrap();
    assert_eq!(
        original.sha256,
        "5a1b3282d02d77352cd7abde306e7457e159de74a07d888d0432fda55ffebdd2"
    );
    assert!(
        serde_json::to_value(&original.settings).unwrap()["layout"]
            .get("scale_percent")
            .is_none()
    );
    r.overrides.scale_percent = Some(100.);
    assert_eq!(label::preview(&r).unwrap().sha256, original.sha256);
    for scale in [50., 150.] {
        r.overrides.scale_percent = Some(scale);
        let changed = label::preview(&r).unwrap();
        assert_ne!(changed.packed, original.packed);
        if scale == 50. {
            assert_eq!(
                changed.sha256,
                "416a63a8fce59b222d3720b13019252e618d2169f30a3408f2d147377ecc3f2b"
            );
        }
        assert_ne!(changed.sha256, original.sha256);
        assert_eq!(
            serde_json::to_value(&changed.settings).unwrap()["layout"]["scale_percent"],
            scale
        );
        let mut snapshot = r.clone();
        snapshot.overrides = Overrides::default();
        snapshot.snapshot = Some(changed.settings);
        assert_eq!(label::preview(&snapshot).unwrap().sha256, changed.sha256);
    }
    for scale in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0., 9.9, 200.1] {
        r.overrides.scale_percent = Some(scale);
        assert_eq!(label::preview(&r).unwrap_err().code, "invalid_settings");
        let mut bad = original.settings.clone();
        bad.layout.scale_percent = scale;
        assert!(bad.validate(&bad.source_sha256.clone()).is_err());
    }
    let mut pattern = Request {
        test_pattern: true,
        overrides: Overrides {
            width_mm: Some(50.),
            height_mm: Some(80.),
            ..Default::default()
        },
        ..Default::default()
    };
    let p = label::preview(&pattern).unwrap();
    assert_eq!(
        p.sha256,
        "d1f2d268a34373a079fc1c35918f9d3ef2686bf9cab26e8e74a7e65191a83410"
    );
    assert_eq!(
        p.input_sha256,
        "ead10037e3ddab7bb8d3c05007bf90ec9e16b1b40cf9748ec52e5a5cc158f029"
    );
    pattern.overrides.scale_percent = Some(100.);
    assert_eq!(label::preview(&pattern).unwrap().sha256, p.sha256);
    pattern.overrides.scale_percent = Some(150.);
    assert_eq!(
        label::preview(&pattern).unwrap_err().code,
        "invalid_settings"
    );
}
#[test]
fn sidecars_are_disjoint_for_all_filename_forms_and_protect_source_aliases() {
    let dir = tempfile::tempdir().unwrap();
    // Exercise both case spellings even on the default case-insensitive macOS volume.
    let source_path = |name: &str| {
        dir.path()
            .join(if name == ".SVG" {
                "uppercase-dotfile"
            } else {
                "."
            })
            .join(name)
    };
    let names = [
        ("logo.svg", "logo.openlabel.json"),
        ("upper.SVG", "upper.openlabel.json"),
        ("logo.png", "logo.png.openlabel-image.json"),
        ("logo.jpg", "logo.jpg.openlabel-image.json"),
        ("photo.JPEG", "photo.JPEG.openlabel-image.json"),
        ("page.tif", "page.tif.openlabel-image.json"),
        ("photo.png", "photo.png.openlabel-image.json"),
        ("photo.png.svg", "photo.png.openlabel.json"),
        ("report", "report.openlabel-image.json"),
        ("report.svg", "report.openlabel.json"),
        (".svg", ".svg.openlabel-image.json"),
        (".SVG", ".SVG.openlabel-image.json"),
    ];
    let mut saved = Vec::new();
    for (i, (name, expected)) in names.iter().enumerate() {
        let path = source_path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, encode(ImageFormat::Png)).unwrap();
        let p =
            label::preview(&request(&path)).unwrap_or_else(|error| panic!("{path:?}: {error:?}"));
        let mut s = p.settings;
        s.printer.density = i as u8 + 1;
        let side = settings::sidecar(&path);
        assert_eq!(side.file_name().unwrap(), *expected);
        assert!(!saved.contains(&side));
        saved.push(side.clone());
        settings::write_output(
            &side,
            &serde_json::to_vec(&s).unwrap(),
            false,
            std::slice::from_ref(&path),
        )
        .unwrap();
        let alias = dir.path().join(format!("alias-{i}"));
        fs::hard_link(&path, &alias).unwrap();
        assert!(
            settings::write_output(&alias, b"oops", true, std::slice::from_ref(&path)).is_err()
        );
    }
    for (i, (name, _)) in names.iter().enumerate() {
        assert_eq!(
            label::preview(&request(&source_path(name)))
                .unwrap()
                .settings
                .printer
                .density,
            i as u8 + 1
        );
    }
}
#[test]
fn alpha_composites_before_triangle_resize_and_centered_fit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("alpha.png");
    for (pixel, threshold, black) in [
        (Rgba([0, 0, 0, 128]), 128, true),
        (Rgba([0, 0, 0, 128]), 127, false),
        (Rgba([0, 255, 0, 0]), 255, false),
    ] {
        RgbaImage::from_pixel(4, 2, pixel).save(&path).unwrap();
        let mut r = request(&path);
        r.overrides.threshold = Some(threshold);
        let p = label::preview(&r).unwrap();
        let bits = |x: usize, y: usize| p.packed[y * 48 + x / 8] & (0x80 >> (x % 8)) != 0;
        assert_eq!(bits(192, 120), black);
        assert!(!bits(192, 8));
    }
    let edge = RgbaImage::from_fn(800, 400, |x, _| {
        if x < 400 {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    });
    edge.save(&path).unwrap();
    let p = label::preview(&request(&path)).unwrap();
    assert_ne!(p.packed[120 * 48 + 10], 0);
    assert_eq!(p.packed[120 * 48 + 40], 0);
    assert_eq!(p.packed[8 * 48 + 10], 0);
    assert_ne!(p.packed[25 * 48 + 10], 0);
    assert_eq!(label::preview(&request(&path)).unwrap().packed, p.packed);
}
#[test]
fn jpeg_orientation_is_applied_before_fitting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rotated.jpg");
    let original = encode(ImageFormat::Jpeg);
    // Minimal EXIF TIFF orientation = 6 (90 degrees clockwise).
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut bytes = original[..2].to_vec();
    bytes.extend([0xff, 0xe1]);
    bytes.extend(((exif.len() + 2) as u16).to_be_bytes());
    bytes.extend(exif);
    bytes.extend(&original[2..]);
    fs::write(&path, bytes).unwrap();
    let Artwork::Bitmap(actual) = label::load(&request(&path)).unwrap().artwork else {
        panic!("bitmap expected")
    };
    let expected =
        image::imageops::rotate90(&image::load_from_memory(&original).unwrap().to_rgba8());
    assert_eq!(actual, expected);
    assert_eq!(actual.dimensions(), (40, 80));
}
#[test]
fn webp_preflight_rejects_tiny_malicious_declarations_before_decoder_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tiny.webp");
    let huge_vp8 = [0x10, 0, 0, 0x9d, 1, 0x2a, 0xff, 0x3f, 0xff, 0x3f];
    let mut frame = vec![0; 16];
    chunk(&mut frame, b"VP8 ", &huge_vp8);
    let mut frame_alpha = vec![0; 16];
    chunk(&mut frame_alpha, b"ALPH", &[0, 255]);
    chunk(&mut frame_alpha, b"VP8 ", &huge_vp8);
    let valid = encode(ImageFormat::WebP);
    let image_data =
        &valid[20..20 + u32::from_le_bytes(valid[16..20].try_into().unwrap()) as usize];
    let small = [0; 10];
    let canvas = [0, 0, 0, 0, 79, 0, 0, 39, 0, 0];
    let animation = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut bad = vec![
        webp(&[(b"VP8X", &small), (b"VP8 ", &huge_vp8)]),
        webp(&[(b"VP8X", &small), (b"VP8L", image_data)]),
        webp(&[(b"VP8X", &animation), (b"ANIM", &[0; 6]), (b"ANMF", &frame)]),
        webp(&[
            (b"VP8X", &animation),
            (b"ANIM", &[0; 6]),
            (b"ANMF", &frame_alpha),
        ]),
        webp(&[
            (b"VP8X", &canvas),
            (b"VP8L", image_data),
            (b"VP8 ", &huge_vp8),
        ]),
        webp(&[
            (b"VP8X", &canvas),
            (b"VP8L", image_data),
            (b"VP8L", image_data),
        ]),
        webp(&[
            (b"VP8X", &animation),
            (b"VP8 ", &huge_vp8),
            (b"ANIM", &[0; 6]),
            (b"ANMF", &frame),
        ]),
    ];
    for declared in [u32::MAX, 100] {
        let mut bytes = webp(&[(b"VP8X", &canvas), (b"VP8L", image_data)]);
        bytes.extend(b"EXIF");
        bytes.extend(declared.to_le_bytes());
        let len = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&len.to_le_bytes());
        bad.push(bytes);
    }
    let mut truncated_padding = webp(&[(b"VP8X", &canvas), (b"VP8L", image_data), (b"EXIF", &[0])]);
    truncated_padding.pop();
    let len = (truncated_padding.len() - 8) as u32;
    truncated_padding[4..8].copy_from_slice(&len.to_le_bytes());
    bad.push(truncated_padding);
    let mut trailing = valid.clone();
    trailing.extend(b"EXIF\xff\xff\xff\xff");
    bad.push(trailing);
    let mut oversized_riff = valid;
    oversized_riff[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    bad.push(oversized_riff);
    for bytes in bad {
        assert!(bytes.len() < 1024);
        fs::write(&path, bytes).unwrap();
        let error = label::preview(&request(&path)).unwrap_err();
        assert_eq!(error.code, "invalid_label");
        assert_eq!(error.message.as_ref().unwrap().key, "err.webpContainer");
    }
}
fn chunk(output: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    output.extend(tag);
    output.extend((payload.len() as u32).to_le_bytes());
    output.extend(payload);
    if !payload.len().is_multiple_of(2) {
        output.push(0);
    }
}
fn webp(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    for (tag, payload) in chunks {
        chunk(&mut body, tag, payload);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
}
#[test]
fn webp_preflight_preserves_lossy_alpha_animation_and_exif() {
    // Synthetic 2x1 image encoded once with cwebp; no runtime external encoder needed.
    let lossy = [
        0x90, 0x01, 0x00, 0x9d, 0x01, 0x2a, 0x02, 0x00, 0x01, 0x00, 0x02, 0x00, 0x34, 0x25, 0xa4,
        0x00, 0x02, 0xe7, 0x59, 0xb6, 0x00, 0x00, 0xfe, 0xf2, 0x76, 0x43, 0xb6, 0x00, 0x00, 0x00,
    ];
    let simple = webp(&[(b"VP8 ", &lossy)]);
    let expected = image::load_from_memory(&simple).unwrap().to_rgba8();
    let mut frame = vec![0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 100, 0, 0, 2];
    chunk(&mut frame, b"VP8 ", &lossy);
    let exif = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut alpha_expected = expected.clone();
    alpha_expected.get_pixel_mut(0, 0)[3] = 128;
    let mut cases = vec![
        (simple, expected.clone()),
        (
            webp(&[
                (b"VP8X", &[0, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
                (b"VP8 ", &lossy),
            ]),
            expected.clone(),
        ),
        (
            webp(&[
                (b"VP8X", &[16, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
                (b"ALPH", &[0, 128, 255]),
                (b"VP8 ", &lossy),
            ]),
            alpha_expected.clone(),
        ),
        (
            webp(&[
                (b"VP8X", &[2, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
                (b"ANIM", &[0; 6]),
                (b"ANMF", &frame),
            ]),
            expected.clone(),
        ),
        (
            webp(&[
                (b"VP8X", &[8, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
                (b"VP8 ", &lossy),
                (b"EXIF", exif),
            ]),
            image::imageops::rotate90(&expected),
        ),
    ];
    let mut alpha_frame = frame[..16].to_vec();
    chunk(&mut alpha_frame, b"ALPH", &[0, 128, 255]);
    chunk(&mut alpha_frame, b"VP8 ", &lossy);
    let mut frame_tail = frame.clone();
    chunk(&mut frame_tail, b"JUNK", &[0]);
    let mut alpha_tail = alpha_frame.clone();
    chunk(&mut alpha_tail, b"JUNK", &[0]);
    for (frame, expected, flags) in [
        (frame_tail, expected, 2),
        (alpha_frame, alpha_expected.clone(), 0x12),
        (alpha_tail, alpha_expected, 0x12),
    ] {
        cases.push((
            webp(&[
                (b"VP8X", &[flags, 0, 0, 0, 1, 0, 0, 0, 0, 0]),
                (b"ANIM", &[0; 6]),
                (b"ANMF", &frame),
            ]),
            expected,
        ));
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("valid.webp");
    for (bytes, expected) in cases {
        fs::write(&path, &bytes).unwrap();
        let loaded = label::load(&request(&path)).unwrap();
        assert_eq!(loaded.source, bytes);
        let Artwork::Bitmap(actual) = loaded.artwork else {
            panic!("bitmap expected")
        };
        assert_eq!(actual, expected);
    }
}
#[test]
fn webp_frame_tails_keep_bounds_and_reject_recognized_chunks() {
    let encoded = encode(ImageFormat::WebP);
    let mut frame = vec![0, 0, 0, 0, 0, 0, 79, 0, 0, 39, 0, 0, 100, 0, 0, 2];
    frame.extend(&encoded[12..]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tail.webp");
    let mut tails = Vec::new();
    for tag in [
        b"RIFF", b"WEBP", b"VP8X", b"VP8 ", b"VP8L", b"ALPH", b"ANIM", b"ANMF", b"EXIF", b"ICCP",
        b"XMP ",
    ] {
        let mut tail = Vec::new();
        chunk(&mut tail, tag, &[0]);
        tails.push(tail);
    }
    tails.push(b"JUNK\x03\0\0\0\0\0".to_vec()); // Truncated payload.
    tails.push(b"JUNK\x01\0\0\0\0".to_vec()); // Missing subchunk pad; ANMF's outer pad is not frame data.
    for tail in tails {
        let mut malformed = frame.clone();
        malformed.extend(tail);
        let bytes = webp(&[
            (b"VP8X", &[2, 0, 0, 0, 79, 0, 0, 39, 0, 0]),
            (b"ANIM", &[0; 6]),
            (b"ANMF", &malformed),
        ]);
        fs::write(&path, bytes).unwrap();
        let error = label::load(&request(&path)).err().unwrap();
        assert_eq!(error.message.as_ref().unwrap().key, "err.webpContainer");
    }
}
#[test]
fn animated_webp_decodes_only_the_first_frame() {
    let first = encode(ImageFormat::WebP);
    let mut second = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(80, 40, Rgb([255, 255, 255])))
        .write_to(&mut second, ImageFormat::WebP)
        .unwrap();
    let mut body = b"WEBP".to_vec();
    chunk(&mut body, b"VP8X", &[2, 0, 0, 0, 79, 0, 0, 39, 0, 0]);
    chunk(&mut body, b"ANIM", &[255, 255, 255, 255, 0, 0]);
    for encoded in [&first, &second.into_inner()] {
        let mut frame = vec![0, 0, 0, 0, 0, 0, 79, 0, 0, 39, 0, 0, 100, 0, 0, 2];
        frame.extend(&encoded[12..]);
        chunk(&mut body, b"ANMF", &frame);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((body.len() as u32).to_le_bytes());
    bytes.extend(body);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("animated.webp");
    fs::write(&path, bytes).unwrap();
    let Artwork::Bitmap(actual) = label::load(&request(&path)).unwrap().artwork else {
        panic!("bitmap expected")
    };
    assert_eq!(actual, image::load_from_memory(&first).unwrap().to_rgba8());
}
#[test]
fn gif_first_frame_and_descriptor_resource_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("animation.gif");
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        encoder
            .encode_frames(
                [
                    RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 255])),
                    RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255])),
                ]
                .into_iter()
                .map(image::Frame::new),
            )
            .unwrap();
    }
    fs::write(&path, &bytes).unwrap();
    let Artwork::Bitmap(first) = label::load(&request(&path)).unwrap().artwork else {
        panic!("bitmap expected")
    };
    assert_eq!(first.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
    // Synthetic descriptor is rejected before LZW data needs decoding.
    for (x, y, w, h) in [
        (0u16, 0u16, 5000u16, 4000u16),
        (0, 0, 16385, 1),
        (1, 0, 2, 2),
        (0, 1, 2, 2),
        (0, 0, 0, 1),
    ] {
        let mut bomb = b"GIF89a\x02\0\x02\0\0\0\0\x2c".to_vec();
        for n in [x, y, w, h] {
            bomb.extend(n.to_le_bytes());
        }
        bomb.extend([0, 2, 0]);
        fs::write(&path, bomb).unwrap();
        assert!(label::preview(&request(&path)).is_err());
    }
    for end in [6, 13, bytes.len() / 2] {
        fs::write(&path, &bytes[..end]).unwrap();
        assert!(label::preview(&request(&path)).is_err());
    }
    fs::write(&path, b"GIF89a\x02\0\x02\0\0\0\0\x3b").unwrap();
    assert!(label::preview(&request(&path)).is_err());
}
// Tiny uncompressed TIFF IFDs: no large sample allocation is needed for resource tests.
fn tiff_page(
    width: u32,
    height: u32,
    samples: u32,
    bits: u16,
    sample_format: u16,
    value: u8,
    second: bool,
) -> Vec<u8> {
    let mut bytes = b"II\x2a\0\x08\0\0\0".to_vec();
    let page_len = 2 + 12 * 12 + 4 + 8 + 8 + 4;
    for page in 0..if second { 2 } else { 1 } {
        let start = 8 + page * page_len;
        let extras = start + 2 + 12 * 12 + 4;
        let data = extras + 16;
        bytes.extend(12u16.to_le_bytes());
        for (tag, kind, count, val) in [
            (256u16, 4u16, 1, width),
            (257, 4, 1, height),
            (
                258,
                3,
                samples,
                if samples == 1 {
                    u32::from(bits)
                } else {
                    extras as u32
                },
            ),
            (259, 3, 1, 1),
            (262, 3, 1, if samples == 1 { 1 } else { 2 }),
            (273, 4, 1, data as u32),
            (277, 3, 1, samples),
            (278, 4, 1, height),
            (
                279,
                4,
                1,
                width
                    .saturating_mul(height)
                    .saturating_mul(samples)
                    .saturating_mul(u32::from(bits) / 8),
            ),
            (284, 3, 1, 1),
            (338, 3, 1, 2),
            (
                339,
                3,
                samples,
                if samples == 1 {
                    u32::from(sample_format)
                } else {
                    (extras + 8) as u32
                },
            ),
        ] {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend(count.to_le_bytes());
            bytes.extend(val.to_le_bytes());
        }
        bytes.extend(
            (if second && page == 0 {
                (start + page_len) as u32
            } else {
                0
            })
            .to_le_bytes(),
        );
        for _ in 0..4 {
            bytes.extend(bits.to_le_bytes());
        }
        for _ in 0..4 {
            bytes.extend(sample_format.to_le_bytes());
        }
        bytes.extend([if page == 0 { value } else { 255 }; 4]);
    }
    bytes
}
#[test]
fn tiff_associated_alpha_is_normalized_at_rgba8_for_both_byte_orders() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("alpha.tiff");
    let alpha_entry = 10 + 10 * 12;
    for big_endian in [false, true] {
        for (extra, input, expected, black) in [
            (1u16, [128, 128, 128, 128], [255, 255, 255, 128], false),
            (2, [255, 255, 255, 128], [255, 255, 255, 128], false),
            (1, [123, 99, 44, 0], [0, 0, 0, 0], false),
            (1, [64, 64, 64, 128], [128, 128, 128, 128], true),
            (1, [25, 50, 75, 255], [25, 50, 75, 255], true),
        ] {
            let mut bytes = tiff_page(1, 1, 4, 8, 1, 0, false);
            bytes[alpha_entry + 8..alpha_entry + 10].copy_from_slice(&extra.to_le_bytes());
            let data = bytes.len() - 4;
            bytes[data..].copy_from_slice(&input);
            if big_endian {
                bytes[..8].copy_from_slice(b"MM\0\x2a\0\0\0\x08");
                bytes[8..10].reverse();
                for entry in bytes[10..154].chunks_exact_mut(12) {
                    let inline_short = entry[2..4] == [3, 0] && entry[4..8] == [1, 0, 0, 0];
                    entry[..2].reverse();
                    entry[2..4].reverse();
                    entry[4..8].reverse();
                    if inline_short {
                        entry[8..10].reverse();
                    } else {
                        entry[8..12].reverse();
                    }
                }
                for value in bytes[158..data].chunks_exact_mut(2) {
                    value.reverse();
                }
            }
            fs::write(&path, &bytes).unwrap();
            let loaded = label::load(&request(&path)).unwrap();
            let Artwork::Bitmap(actual) = loaded.artwork else {
                panic!("bitmap expected")
            };
            assert_eq!(actual.get_pixel(0, 0).0, expected);
            let mut r = request(&path);
            r.overrides.threshold = Some(200);
            let preview = label::preview(&r).unwrap();
            assert_eq!(preview.source_sha256, openlabel_core::sha256(&bytes));
            assert_eq!(preview.packed[120 * 48 + 24] & 0x80 != 0, black);
        }
    }
    for (offset, replacement) in [
        (alpha_entry + 2, vec![4, 0]),
        (alpha_entry + 4, vec![2, 0, 0, 0]),
        (alpha_entry + 8, vec![3, 0]),
        (4, u32::MAX.to_le_bytes().to_vec()),
    ] {
        let mut bytes = tiff_page(1, 1, 4, 8, 1, 0, false);
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        fs::write(&path, bytes).unwrap();
        let error = label::preview(&request(&path)).unwrap_err();
        assert_eq!(error.message.as_ref().unwrap().key, "err.tiffSamples");
    }
}
#[test]
fn tiff_first_page_and_high_bit_depth_output_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pages.tiff");
    fs::write(&path, tiff_page(1, 1, 1, 8, 1, 0, true)).unwrap();
    let Artwork::Bitmap(first) = label::load(&request(&path)).unwrap().artwork else {
        panic!("bitmap expected")
    };
    assert_eq!(first.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
    fs::write(&path, tiff_page(3072, 3072, 4, 32, 3, 0, false)).unwrap();
    let error = label::preview(&request(&path)).unwrap_err();
    assert_eq!(error.message.as_ref().unwrap().key, "err.imageMemory");
}
#[test]
fn corrupt_unsupported_bytes_and_pixel_bombs_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input");
    for bytes in [
        vec![0; 16 * 1024 * 1024 + 1],
        b"%PDF-1.7".to_vec(),
        b"\0\0\0\x18ftypavif".to_vec(),
        b"not an image".to_vec(),
        b"\x89PNG\r\n\x1a\n".to_vec(),
    ] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            label::preview(&request(&path)).unwrap_err().code,
            "invalid_label"
        );
    }
    for (w, h) in [(16385, 1), (4001, 4000)] {
        let mut bytes = Vec::new();
        {
            let encoder = png::Encoder::new(&mut bytes, w, h);
            let _writer = encoder.write_header().unwrap();
        }
        fs::write(&path, bytes).unwrap();
        assert!(label::preview(&request(&path)).is_err());
    }
    fs::write(
        &path,
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='40mm' height='30mm'>{}</svg>",
            " ".repeat(1024 * 1024)
        ),
    )
    .unwrap();
    assert!(
        label::preview(&request(&path))
            .unwrap_err()
            .detail
            .contains("1MiB")
    );
}
