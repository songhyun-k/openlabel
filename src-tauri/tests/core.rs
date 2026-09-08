use base64::Engine;
use openlabel_core::{
    ble,
    label::{self, Request},
    m110,
    raster::{Geometry, png_from_bits},
    settings::{self, Overrides, Settings},
};
use std::{fs, path::PathBuf};
fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/sample.svg")
}
fn request() -> Request {
    Request {
        path: Some(sample()),
        defaults: true,
        ..Default::default()
    }
}
fn svg(body: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40mm\" height=\"30mm\" viewBox=\"0 0 40 30\">{body}</svg>"
    )
}
#[test]
fn settings_boundaries_and_precedence() {
    let hash = "0".repeat(64);
    let good = Settings::defaults(&hash, 40., 30.);
    for (key, value) in [
        ("width_mm", 19.9),
        ("width_mm", 50.1),
        ("height_mm", 9.9),
        ("height_mm", 100.1),
        ("margin_mm", -0.1),
        ("margin_mm", 5.1),
        ("offset_x_mm", 10.1),
        ("offset_y_mm", -10.1),
        ("density", 0.),
        ("density", 16.),
        ("speed", 0.),
        ("speed", 6.),
        ("rotation_deg", 45.),
    ] {
        let mut s = good.clone();
        let group = match key {
            "width_mm" | "height_mm" => "paper",
            "density" | "speed" => "printer",
            _ => "layout",
        };
        let mut v = serde_json::to_value(&s).unwrap();
        v[group][key] = serde_json::json!(value);
        if let Ok(changed) = serde_json::from_value(v) {
            s = changed;
            assert!(s.validate(&hash).is_err(), "{key}={value}");
        }
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut s = good.clone();
        s.paper.width_mm = value;
        assert!(s.validate(&hash).is_err());
    }
    for angle in [0, 90, 180, 270, 45, 89, 360] {
        let mut s = good.clone();
        s.layout.rotation_deg = angle;
        assert_eq!(
            s.validate(&hash).is_ok(),
            [0, 90, 180, 270].contains(&angle)
        );
    }
    for angle in [serde_json::json!(-90), serde_json::json!(90.5)] {
        let mut value = serde_json::to_value(&good).unwrap();
        value["layout"]["rotation_deg"] = angle;
        assert!(serde_json::from_value::<Settings>(value).is_err());
    }
    let mut s = good.clone();
    s.paper.height_mm = 10.;
    s.layout.margin_mm = 5.;
    assert!(s.validate(&hash).is_err());
    for (w, h) in [(20., 10.), (50., 100.)] {
        let mut s = Settings::defaults(&hash, w, h);
        assert!(s.validate(&hash).is_ok());
    }
    let mut s = good.clone();
    s.layout.alignment = "auto".into();
    assert!(s.validate(&hash).is_err());
    s = good.clone();
    s.raster.mode = "unknown".into();
    assert!(s.validate(&hash).is_err());
    let mut v = serde_json::to_value(&good).unwrap();
    v["copies"] = 1.into();
    assert!(serde_json::from_value::<Settings>(v).is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("label.svg");
    fs::copy(sample(), &path).unwrap();
    let mut r = Request {
        path: Some(path.clone()),
        ..Default::default()
    };
    let initial = label::preview(&r).unwrap();
    let mut side = initial.settings.clone();
    side.printer.density = 4;
    fs::write(settings::sidecar(&path), serde_json::to_vec(&side).unwrap()).unwrap();
    assert_eq!(label::preview(&r).unwrap().settings.printer.density, 4);
    r.overrides.density = Some(5);
    assert_eq!(label::preview(&r).unwrap().settings.printer.density, 5);
    r.defaults = true;
    r.overrides = Overrides::default();
    assert_eq!(label::preview(&r).unwrap().settings.printer.density, 10);
    r.settings = Some(settings::sidecar(&path));
    assert!(label::preview(&r).is_err());
}

#[test]
fn source_size_defaults_fit_before_paper_overrides_but_sidecars_stay_strict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.svg");
    for width in [5., 60., 100.] {
        fs::write(&path,format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}mm\" height=\"30mm\"><rect width=\"10\" height=\"10\"/></svg>")).unwrap();
        let mut r = Request {
            path: Some(path.clone()),
            ..Default::default()
        };
        let initial = label::preview(&r).unwrap();
        assert_eq!(
            (
                initial.settings.paper.width_mm,
                initial.settings.paper.height_mm
            ),
            (50., 30.)
        );
        r.overrides.width_mm = Some(40.);
        let p = label::preview(&r).unwrap();
        assert_eq!(p.geometry.paper_width, 320);
        let mut side = p.settings;
        side.paper.width_mm = 60.;
        fs::write(settings::sidecar(&path), serde_json::to_vec(&side).unwrap()).unwrap();
        assert_eq!(label::preview(&r).unwrap_err().code, "invalid_settings");
        fs::remove_file(settings::sidecar(&path)).unwrap();
    }
}
#[test]
fn svg_contract_rejects_resources_and_complexity() {
    for body in [
        "<script/>",
        "<foreignObject/>",
        "<use href=\"#x\"/>",
        "<style>rect{fill:black}</style>",
        "<rect onclick=\"x\"/>",
        "<image href=\"https://bad/image.png\"/>",
        "<image href=\"&#x66;ile:///tmp/x.png\"/>",
        "<image href=\"data:image/svg+xml;base64,PHN2Zy8+\"/>",
        "<rect fill=\"url(#x)\"/>",
        "<rect style=\"fill:u\\72l(file:///x)\"/>",
        "<rect style=\"@import:url(x)\"/>",
        "<image href=\"data:image/png;base64,AAAA\"/>",
        "<animate/>",
    ] {
        assert!(label::validate_svg(svg(body).as_bytes()).is_err(), "{body}");
    }
    assert!(
        label::validate_svg(b"<!DOCTYPE svg [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><svg/>")
            .is_err()
    );
    assert!(label::validate_svg(svg(&"<rect/>".repeat(4096)).as_bytes()).is_err());
    assert!(
        label::validate_svg(svg(&format!("{}{}", "<g>".repeat(33), "</g>".repeat(33))).as_bytes())
            .is_err()
    );
    assert!(
        label::validate_svg(svg(&format!("<path d=\"{}\"/>", "M0 0 ".repeat(27000))).as_bytes())
            .is_err()
    );
    assert!(label::validate_svg(&vec![b' '; 1024 * 1024 + 1]).is_err());
    let mut png = Vec::new();
    {
        let mut e = png::Encoder::new(&mut png, 2001, 2000);
        e.set_color(png::ColorType::Grayscale);
        let mut writer = e.write_header().unwrap();
        writer.write_image_data(&vec![255; 2001 * 2000]).unwrap();
    }
    let bomb = svg(&format!(
        "<image width=\"10\" height=\"10\" href=\"data:image/png;base64,{}\"/>",
        base64::engine::general_purpose::STANDARD.encode(png)
    ));
    assert!(label::validate_svg(bomb.as_bytes()).is_err());
}

#[test]
fn svg_render_inline_png_and_jpeg() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.svg");
    for (mime, format) in [
        ("png", image::ImageFormat::Png),
        ("jpeg", image::ImageFormat::Jpeg),
    ] {
        let pixels = image::RgbImage::from_pixel(2, 2, image::Rgb([0, 0, 0]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut encoded, format)
            .unwrap();
        let document = svg(&format!(
            "<image x=\"5\" y=\"5\" width=\"20\" height=\"20\" href=\"data:image/{mime};base64,{}\"/>",
            base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())
        ));
        fs::write(&path, document).unwrap();
        let p = label::preview(&Request {
            path: Some(path.clone()),
            ..Default::default()
        })
        .unwrap();
        assert!(p.packed.iter().map(|b| b.count_ones()).sum::<u32>() > 1000);
    }
}
#[test]
fn geometry_table_and_signed_rounding() {
    for (w, h, paper, x, cw, ch) in [
        (30., 20., 240, 72, 224, 144),
        (40., 30., 320, 32, 304, 224),
        (50., 30., 400, -8, 384, 224),
        (30.125, 20., 241, 71, 225, 144),
    ] {
        let s = Settings::defaults(&"0".repeat(64), w, h);
        let g = Geometry::new(&s).unwrap();
        assert_eq!(
            (g.paper_width, g.paper_x, g.content_width, g.content_height),
            (paper, x, cw, ch)
        );
    }
    for (mm, n) in [
        (0.0625, 1),
        (-0.0625, -1),
        (0.125, 1),
        (-0.125, -1),
        (0.5, 4),
        (-0.5, -4),
    ] {
        assert_eq!(settings::dots(mm), n);
    }
}
#[test]
fn preview_identity_png_text_and_transforms() {
    let original = label::preview(&request()).unwrap();
    let decoded = image::load_from_memory(&original.png).unwrap().to_luma8();
    for (i, p) in decoded.pixels().enumerate() {
        assert_eq!(p[0] == 0, original.packed[i / 8] & (0x80 >> (i % 8)) != 0);
    }
    assert!(original.packed.iter().any(|b| *b != 0));
    let mut r = request();
    r.overrides.density = Some(1);
    r.overrides.speed = Some(5);
    let changed = label::preview(&r).unwrap();
    assert_eq!(original.sha256, changed.sha256);
    assert_eq!(original.input_sha256, changed.input_sha256);
    r.overrides.mirror = Some(true);
    assert_ne!(original.sha256, label::preview(&r).unwrap().sha256);
    r.overrides.rotation_deg = Some(180);
    assert_eq!(
        label::preview(&r).unwrap().sha256,
        "da83ae695b8bb430b361e77938eb44406c18f58120b858232a528b92e3811335"
    );
    r.overrides.mirror = Some(false);
    assert_eq!(
        label::preview(&r).unwrap().sha256,
        "256a37ef85c83c82613260bf860ea88353c63b545f2c8a8fbda25c1a655fa0ee"
    );
    r.overrides.offset_x_mm = Some(10.);
    assert!(label::preview(&r).unwrap().clipped_dot_count > 0);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("text.svg");
    fs::write(&path,svg("<text x=\"2\" y=\"15\" font-family=\"Nanum Gothic\" font-size=\"5\">한글 Openlabel</text>")).unwrap();
    let text = label::preview(&Request {
        path: Some(path),
        ..Default::default()
    })
    .unwrap();
    assert!(
        text.packed.iter().map(|b| b.count_ones()).sum::<u32>() > 100,
        "bundled Korean/Latin text must not disappear"
    );
    assert_eq!(
        original.sha256,
        "5a1b3282d02d77352cd7abde306e7457e159de74a07d888d0432fda55ffebdd2"
    );
}
#[test]
fn test_raster_pattern_ruler_uses_40_dots() {
    for width in [30., 40., 50.] {
        for margin in [0., 1., 3.] {
            let r = Request {
                test_pattern: true,
                overrides: Overrides {
                    width_mm: Some(width),
                    margin_mm: Some(margin),
                    ..Default::default()
                },
                ..Default::default()
            };
            let p = label::preview(&r).unwrap();
            let g = p.geometry;
            let black = |x: i32, y: i32| {
                p.packed[y as usize * 48 + x as usize / 8] & (0x80 >> (x % 8)) != 0
            };
            for x in (40..g.content_width - 20).step_by(40) {
                assert!(
                    black(g.content_x + x - 1, g.content_y + 4)
                        || black(g.content_x + x, g.content_y + 4)
                );
                assert!(!black(g.content_x + x + 5, g.content_y + 4));
            }
        }
    }
}
#[test]
fn m110_protocol_golden_and_gatt_properties() {
    let bits = vec![0x81; 144];
    let p = settings::Printer {
        density: 10,
        speed: 1,
    };
    let steps = m110::sequence(&bits, 3, &p).unwrap();
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("../../fixtures/protocol/m110-v1.json")).unwrap();
    for (step, row) in steps.iter().zip(golden["writes"].as_array().unwrap()) {
        assert_eq!(step.delay_ms, row["delay_ms"].as_u64().unwrap());
        if let Some(hex) = row["hex"].as_str() {
            assert_eq!(
                step.bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                hex
            );
        } else {
            assert_eq!(
                step.bytes,
                vec![
                    row["repeat_byte"].as_u64().unwrap() as u8;
                    row["count"].as_u64().unwrap() as usize
                ]
            );
        }
    }
    assert_eq!(
        steps.iter().map(|s| s.delay_ms).collect::<Vec<_>>(),
        [30, 30, 30, 0, 20, 20, 300, 500]
    );
    assert_eq!(steps[0].bytes, [27, 78, 13, 1]);
    assert_eq!(steps[1].bytes, [27, 78, 4, 10]);
    assert_eq!(steps[2].bytes, [31, 17, 10]);
    assert_eq!(steps[3].bytes, [29, 118, 48, 0, 48, 0, 3, 0]);
    assert_eq!(steps[4].bytes, vec![0x81; 128]);
    assert_eq!(steps[5].bytes, vec![0x81; 16]);
    assert_eq!(steps[7].bytes, [31, 240, 5, 0, 31, 240, 3, 0]);
    assert!(m110::sequence(&bits, 4, &p).is_err());
    assert!(png_from_bits(&bits, 4).is_err());
    use btleplug::api::{CharPropFlags as F, WriteType as W};
    assert_eq!(
        ble::choose_write(F::WRITE_WITHOUT_RESPONSE).unwrap(),
        W::WithoutResponse
    );
    assert_eq!(ble::choose_write(F::WRITE).unwrap(), W::WithResponse);
    assert_eq!(
        ble::choose_write(F::WRITE | F::WRITE_WITHOUT_RESPONSE).unwrap(),
        W::WithoutResponse
    );
    assert!(ble::choose_write(F::NOTIFY).is_err());
    for name in ["M110S", "M120", "M220", "NIIMBOT D110", "Brother"] {
        assert!(!ble::model_allowed(Some(name)));
    }
    assert!(ble::model_allowed(Some("M110_ABCD")));
}

#[test]
fn desktop_capabilities_are_local_and_no_direct_fs_api() {
    let permissions: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    let permissions = permissions["permissions"].as_array().unwrap();
    assert!(
        !permissions
            .iter()
            .any(|p| p.as_str().unwrap().starts_with("fs:"))
    );
    assert!(
        permissions
            .iter()
            .filter_map(|p| p.as_str())
            .filter(|p| p.starts_with("dialog:"))
            .all(|p| ["dialog:allow-open", "dialog:allow-save"].contains(&p))
    );
    assert!(!include_str!("../src/main.rs").contains("tauri_plugin_fs::init"));
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(
        config["app"]["security"]["csp"],
        "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src ipc: http://ipc.localhost"
    );
}
#[test]
fn declared_layout_bounds_canvas_and_inspector_without_claiming_visual_acceptance() {
    let css = include_str!("../../src/styles.css");
    for rule in [
        "height: 100dvh",
        "grid-template-columns: minmax(0, 1fr) 300px",
        ".preview-scroll { display: flex; overflow: auto;",
        "flex-shrink: 0; margin: auto",
        ".inspector-scroll { overflow: auto; min-height: 0",
        "@media (max-width: 850px)",
        "grid-template-columns: minmax(0, 1fr);",
        "background: var(--background)",
    ] {
        assert!(css.contains(rule), "missing layout rule: {rule}");
    }
}
#[test]
fn settings_output_alias_and_input_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.svg");
    fs::copy(sample(), &source).unwrap();
    let before = fs::read(&source).unwrap();
    let alias = dir.path().join("alias");
    fs::hard_link(&source, &alias).unwrap();
    assert!(settings::write_output(&alias, b"oops", true, std::slice::from_ref(&source)).is_err());
    assert_eq!(before, fs::read(&source).unwrap());
    let out = dir.path().join("out");
    settings::write_output(&out, b"ok", false, &[]).unwrap();
    assert_eq!(
        settings::write_output(&out, b"no", false, &[])
            .unwrap_err()
            .code,
        "output_exists"
    );
    assert_ne!(
        settings::input_hash(b"svg", None),
        settings::input_hash(b"svg", Some(b""))
    );
    assert_ne!(
        settings::input_hash(b"a", Some(b"bc")),
        settings::input_hash(b"ab", Some(b"c"))
    );
}
