use serde_json::Value;
use std::{fs, process::Command};
static PRINT_FLOW: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn cli(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_openlabel"))
        .env("OPENLABEL_TEST_TRANSIENT", "1")
        .args(args)
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)));
    assert_eq!(value["version"], 1);
    assert_eq!(output.status.success(), value["ok"].as_bool().unwrap());
    assert_eq!(
        output.status.code(),
        Some(if value["ok"] == true {
            0
        } else if value["error"]["code"] == "invalid_arguments" {
            2
        } else {
            1
        })
    );
    value
}
#[test]
fn quarter_turn_preview_roundtrip_is_stateless_and_rejects_invalid_angles() {
    use openlabel_core::{
        label::{self, Request},
        settings::{Overrides, Settings},
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("label.svg");
    fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/sample.svg"),
        &source,
    )
    .unwrap();
    for angle in [90, 270] {
        let output = dir.path().join(format!("{angle}.png"));
        let side = dir.path().join(format!("{angle}.json"));
        let again = dir.path().join(format!("{angle}-again.png"));
        let result = cli(&[
            "preview",
            source.to_str().unwrap(),
            "--rotation-deg",
            &angle.to_string(),
            "--width-mm",
            "50",
            "--height-mm",
            "80",
            "--output",
            output.to_str().unwrap(),
            "--settings-out",
            side.to_str().unwrap(),
        ]);
        assert_eq!(result["ok"], true, "{result}");
        let exported: Settings = serde_json::from_slice(&fs::read(&side).unwrap()).unwrap();
        assert_eq!(exported.layout.rotation_deg, angle);
        assert_eq!(
            (exported.paper.width_mm, exported.paper.height_mm),
            (50., 80.)
        );
        let core = label::preview(&Request {
            path: Some(source.clone()),
            settings: Some(side.clone()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            result["result"]["settings"],
            serde_json::to_value(&core.settings).unwrap()
        );
        assert_eq!(result["result"]["sha256"], core.sha256);
        assert_eq!(result["result"]["input_sha256"], core.input_sha256);
        assert_eq!(fs::read(&output).unwrap(), core.png);
        let roundtrip = cli(&[
            "preview",
            source.to_str().unwrap(),
            "--settings",
            side.to_str().unwrap(),
            "--output",
            again.to_str().unwrap(),
        ]);
        for key in ["sha256", "input_sha256", "settings"] {
            assert_eq!(roundtrip["result"][key], result["result"][key]);
        }
        assert_eq!(fs::read(again).unwrap(), core.png);
        let pixels = image::load_from_memory(&core.png).unwrap().to_luma8();
        assert_eq!(pixels.dimensions(), (384, 640));
        for (i, pixel) in pixels.pixels().enumerate() {
            assert_eq!(pixel[0] == 0, core.packed[i / 8] & (0x80 >> (i % 8)) != 0);
        }
        let explicit = label::preview(&Request {
            path: Some(source.clone()),
            defaults: true,
            overrides: Overrides {
                rotation_deg: Some(angle),
                width_mm: Some(50.),
                height_mm: Some(80.),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        assert_eq!(explicit.sha256, core.sha256);
    }
    for angle in ["45", "89", "360", "-90", "90.5"] {
        let output = dir.path().join(format!("invalid-{angle}.png"));
        let result = cli(&[
            "preview",
            source.to_str().unwrap(),
            &format!("--rotation-deg={angle}"),
            "--output",
            output.to_str().unwrap(),
        ]);
        assert_eq!(result["ok"], false);
        assert_eq!(
            result["error"]["code"],
            if ["-90", "90.5"].contains(&angle) {
                "invalid_arguments"
            } else {
                "invalid_settings"
            }
        );
        assert!(!output.exists());
    }
}

#[test]
fn webp_preflight_rejection_is_returned_by_the_render_worker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tiny.webp");
    let output = dir.path().join("out.png");
    let mut bytes = b"RIFF\x28\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
    bytes.extend([0; 10]);
    bytes.extend(b"VP8 \x0a\0\0\0");
    bytes.extend([0x10, 0, 0, 0x9d, 1, 0x2a, 0xff, 0x3f, 0xff, 0x3f]);
    fs::write(&path, bytes).unwrap();
    let result = cli(&[
        "preview",
        path.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert_eq!(result["error"]["code"], "invalid_label");
    assert!(
        result["error"]["detail"]
            .as_str()
            .unwrap()
            .starts_with("WebP preflight:")
    );
    assert!(!output.exists());
    // Small forged metadata only; the 1GiB declaration is confined to a preflight-only unit test.
    let mut bytes = b"RIFF\x5e\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
    bytes.extend([0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    bytes.extend(b"ANIM\x06\0\0\0");
    bytes.extend([0; 6]);
    bytes.extend(b"ANMF\x32\0\0\0");
    bytes.extend([0; 16]);
    bytes.extend(b"ALPH\x08\0\0\0EXIF\x10\0\0\0VP8 \x0a\0\0\0");
    bytes.extend([0x10, 0, 0, 0x9d, 1, 0x2a, 1, 0, 1, 0]);
    fs::write(&path, bytes).unwrap();
    let result = cli(&[
        "preview",
        path.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert_eq!(result["error"]["code"], "invalid_label");
    assert!(
        result["error"]["detail"]
            .as_str()
            .unwrap()
            .starts_with("WebP preflight:")
    );
    assert!(!output.exists());
}
#[test]
fn cli_json_help_error_preview_roundtrip_and_stale() {
    let _serial = PRINT_FLOW.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(cli(&["--help"])["ok"], true);
    assert_eq!(cli(&["nonsense"])["error"]["code"], "invalid_arguments");
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("label.svg");
    fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/sample.svg"),
        &source,
    )
    .unwrap();
    let path = source.to_str().unwrap();
    let output = dir.path().join("out.png");
    let out = output.to_str().unwrap();
    for key in [
        "--scale-percent",
        "--width-mm",
        "--height-mm",
        "--margin-mm",
        "--offset-x-mm",
        "--offset-y-mm",
    ] {
        for value in ["NaN", "inf", "-inf"] {
            let option = format!("{key}={value}");
            assert_eq!(
                cli(&["preview", path, "--output", out, &option])["error"]["code"],
                "invalid_settings"
            );
        }
    }
    let settings = dir.path().join("export.json");
    let side = settings.to_str().unwrap();
    let preview = cli(&[
        "preview",
        path,
        "--output",
        out,
        "--offset-x-mm",
        "-0.5",
        "--raster-mode",
        "floyd-steinberg",
        "--density",
        "12",
        "--settings-out",
        side,
    ]);
    assert_eq!(preview["ok"], true, "{preview}");
    let p = &preview["result"];
    let again = cli(&[
        "preview",
        path,
        "--settings",
        side,
        "--output",
        out,
        "--overwrite",
    ]);
    assert_eq!(again["result"]["sha256"], p["sha256"]);
    assert_eq!(again["result"]["input_sha256"], p["input_sha256"]);
    for field in ["density", "speed"] {
        let original = fs::read(&settings).unwrap();
        let mut changed: Value = serde_json::from_slice(&original).unwrap();
        changed["printer"][field] = 2.into();
        fs::write(&settings, serde_json::to_vec(&changed).unwrap()).unwrap();
        let failed = cli(&[
            "print",
            path,
            "--settings",
            side,
            "--device",
            "never-connect-to-this",
            "--model",
            "M110",
            "--expect-sha256",
            p["sha256"].as_str().unwrap(),
            "--expect-input-sha256",
            p["input_sha256"].as_str().unwrap(),
        ]);
        assert_eq!(failed["error"]["code"], "hash_mismatch", "{failed}");
        fs::write(&settings, original).unwrap();
    }
    let baseline = cli(&[
        "preview",
        path,
        "--defaults",
        "--output",
        out,
        "--overwrite",
    ]);
    let b = &baseline["result"];
    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            cli(&[
                "print",
                path,
                "--defaults",
                "--device",
                "malformed",
                "--model",
                "M110",
                "--expect-sha256",
                b["sha256"].as_str().unwrap(),
                "--expect-input-sha256",
                b["input_sha256"].as_str().unwrap()
            ])["error"]["code"],
            "invalid_arguments"
        );
        assert_eq!(
            cli(&["test-print", "--device", "malformed", "--model", "M110"])["error"]["code"],
            "invalid_arguments"
        );
        assert_eq!(
            cli(&[
                "test-print",
                "--device",
                "malformed",
                "--model",
                "M110",
                "--expect-sha256",
                &"0".repeat(64)
            ])["error"]["code"],
            "hash_mismatch"
        );
    }
    let mut bytes = fs::read(&source).unwrap();
    bytes.push(b'\n');
    fs::write(&source, bytes).unwrap();
    let fresh = cli(&[
        "preview",
        path,
        "--defaults",
        "--output",
        out,
        "--overwrite",
    ]);
    assert_eq!(fresh["result"]["sha256"], b["sha256"]);
    assert_ne!(fresh["result"]["input_sha256"], b["input_sha256"]);
    let failed = cli(&[
        "print",
        path,
        "--defaults",
        "--device",
        "never-connect-to-this",
        "--model",
        "M110",
        "--expect-sha256",
        b["sha256"].as_str().unwrap(),
        "--expect-input-sha256",
        b["input_sha256"].as_str().unwrap(),
    ]);
    assert_eq!(failed["error"]["code"], "hash_mismatch");
    assert_eq!(
        cli(&["preview", path, "--output", out])["error"]["code"],
        "output_exists"
    );
    let before = fs::read(&source).unwrap();
    assert_eq!(
        cli(&["preview", path, "--output", path, "--overwrite"])["error"]["code"],
        "output_error"
    );
    assert_eq!(fs::read(&source).unwrap(), before);
}

#[test]
fn image_formats_scale_worker_roundtrip_and_stale_before_bluetooth() {
    let _serial = PRINT_FLOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("image.data");
    let out = dir.path().join("preview.png");
    let side = dir.path().join("settings.json");
    let (path, output, settings_path) = (
        source.to_str().unwrap(),
        out.to_str().unwrap(),
        side.to_str().unwrap(),
    );
    for format in [
        image::ImageFormat::Png,
        image::ImageFormat::Jpeg,
        image::ImageFormat::WebP,
        image::ImageFormat::Bmp,
        image::ImageFormat::Gif,
        image::ImageFormat::Tiff,
    ] {
        let pixels = image::RgbImage::from_fn(20, 10, |x, _| {
            if x < 10 {
                image::Rgb([0, 0, 0])
            } else {
                image::Rgb([255, 255, 255])
            }
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut encoded, format)
            .unwrap();
        let bytes = encoded.into_inner();
        fs::write(&source, &bytes).unwrap();
        let first = cli(&[
            "preview",
            path,
            "--output",
            output,
            "--overwrite",
            "--scale-percent",
            "75.5",
            "--settings-out",
            settings_path,
        ]);
        assert_eq!(first["ok"], true, "{first}");
        assert_eq!(
            first["result"]["source_sha256"],
            openlabel_core::sha256(&bytes)
        );
        assert_eq!(first["result"]["settings"]["layout"]["scale_percent"], 75.5);
        let again = cli(&[
            "preview",
            path,
            "--output",
            output,
            "--overwrite",
            "--settings",
            settings_path,
        ]);
        for key in ["sha256", "input_sha256", "png_base64"] {
            assert_eq!(first["result"][key], again["result"][key]);
        }
        let stale = cli(&[
            "print",
            path,
            "--settings",
            settings_path,
            "--scale-percent",
            "50",
            "--device",
            "never-connect-to-this",
            "--model",
            "M110",
            "--expect-sha256",
            first["result"]["sha256"].as_str().unwrap(),
            "--expect-input-sha256",
            first["result"]["input_sha256"].as_str().unwrap(),
        ]);
        assert_eq!(stale["error"]["code"], "hash_mismatch", "{stale}");
        assert_eq!(
            cli(&["preview", path, "--output", path, "--overwrite"])["error"]["code"],
            "output_error"
        );
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
    for value in ["0", "9.9", "200.1", "NaN", "inf", "-inf"] {
        let option = format!("--scale-percent={value}");
        assert_eq!(
            cli(&["preview", path, "--output", output, "--overwrite", &option])["error"]["code"],
            "invalid_settings"
        );
    }
    assert_eq!(
        cli(&[
            "preview",
            "--test-pattern",
            "--output",
            output,
            "--overwrite",
            "--scale-percent",
            "150"
        ])["error"]["code"],
        "invalid_settings"
    );
    assert_eq!(
        cli(&[
            "test-print",
            "--device",
            "never-connect-to-this",
            "--model",
            "M110",
            "--scale-percent",
            "150"
        ])["error"]["code"],
        "invalid_settings"
    );
    let golden = cli(&[
        "preview",
        "--test-pattern",
        "--width-mm",
        "50",
        "--height-mm",
        "80",
        "--output",
        output,
        "--overwrite",
    ]);
    assert_eq!(
        golden["result"]["sha256"],
        "d1f2d268a34373a079fc1c35918f9d3ef2686bf9cab26e8e74a7e65191a83410"
    );
    assert_eq!(
        golden["result"]["input_sha256"],
        "ead10037e3ddab7bb8d3c05007bf90ec9e16b1b40cf9748ec52e5a5cc158f029"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_argv_returns_json_instead_of_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let output = Command::new(env!("CARGO_BIN_EXE_openlabel"))
        .env("OPENLABEL_TEST_TRANSIENT", "1")
        .arg(std::ffi::OsString::from_vec(vec![0xff]))
        .output()
        .unwrap();
    let value: Value =
        serde_json::from_slice(&output.stdout).expect("invalid argv must still produce JSON");
    assert_eq!(value["error"]["code"], "invalid_arguments");
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

#[test]
fn cli_large_source_settings_export_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("wide.svg");
    let sidecar = dir.path().join("settings.json");
    let out = dir.path().join("preview.png");
    fs::write(&source,"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100mm\" height=\"5mm\"><rect width=\"10\" height=\"10\"/></svg>").unwrap();
    let first = cli(&[
        "preview",
        source.to_str().unwrap(),
        "--width-mm",
        "40",
        "--height-mm",
        "30",
        "--settings-out",
        sidecar.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
    ]);
    assert_eq!(first["ok"], true, "{first}");
    let second = cli(&[
        "preview",
        source.to_str().unwrap(),
        "--settings",
        sidecar.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
        "--overwrite",
    ]);
    assert_eq!(second["result"]["sha256"], first["result"]["sha256"]);
    assert_eq!(
        second["result"]["input_sha256"],
        first["result"]["input_sha256"]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn cli_render_signals_kill_and_reap_before_json() {
    let _serial = PRINT_FLOW.lock().unwrap_or_else(|e| e.into_inner());
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    for signal in [libc::SIGINT, libc::SIGTERM] {
        for (command, phase) in [
            ("preview", 1),
            ("preview", 2),
            ("test-print", 1),
            ("print", 1),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("label.svg");
            let out = dir.path().join("out.png");
            let sidecar = dir.path().join("settings.json");
            fs::write(&source,format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40mm\" height=\"30mm\"><text x=\"1\" y=\"10\">{}</text></svg>","A".repeat(6000))).unwrap();
            let mut invocation = Command::new(env!("CARGO_BIN_EXE_openlabel"));
            invocation.env("OPENLABEL_TEST_TRANSIENT", "1");
            invocation.arg(command);
            if command == "preview" {
                invocation
                    .arg(&source)
                    .arg("--output")
                    .arg(&out)
                    .arg("--settings-out")
                    .arg(&sidecar);
            } else if command == "test-print" {
                // A missed child observation must fail closed before any BLE operation.
                invocation.args(["--device", "not-a-device", "--model", "not-M110"]);
            } else {
                // A missed cancellation still fails the expected hashes before BLE.
                invocation.arg(&source).args([
                    "--device",
                    "not-a-device",
                    "--model",
                    "M110",
                    "--expect-sha256",
                    &"0".repeat(64),
                    "--expect-input-sha256",
                    &"0".repeat(64),
                ]);
            }
            let mut parent = invocation
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let parent_id = parent.id();
            let mut seen = Vec::new();
            let mut stopped = None;
            let until = Instant::now() + Duration::from_secs(8);
            while Instant::now() < until && parent.try_wait().unwrap().is_none() {
                // Query only our CLI's children without spawning a slower pgrep process.
                let mut children = [0i32; 16];
                let count = unsafe {
                    libc::proc_listchildpids(
                        parent_id as i32,
                        children.as_mut_ptr().cast(),
                        std::mem::size_of_val(&children) as i32,
                    )
                };
                for id in children
                    .into_iter()
                    .take(count.max(0) as usize)
                    .filter(|id| *id > 0)
                {
                    if !seen.contains(&id) {
                        seen.push(id);
                    }
                    if seen.len() == phase && unsafe { libc::kill(id, libc::SIGSTOP) } == 0 {
                        stopped = Some(id);
                        break;
                    }
                }
                if stopped.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let sidecar_before = fs::read(&sidecar).ok();
            unsafe {
                libc::kill(parent_id as i32, signal);
            }
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until && parent.try_wait().unwrap().is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            let bounded = parent.try_wait().unwrap().is_some();
            if !bounded {
                let _ = parent.kill();
            }
            let worker_alive = stopped.is_some_and(|pid| unsafe { libc::kill(pid, 0) } == 0);
            if worker_alive {
                unsafe {
                    libc::kill(stopped.unwrap(), libc::SIGKILL);
                }
            }
            let output = parent.wait_with_output().unwrap();
            assert!(
                stopped.is_some(),
                "did not observe {command} render phase {phase}"
            );
            assert!(bounded, "cancel must finish bounded cleanup");
            assert!(!worker_alive, "render worker survived cancelled parent");
            let value: Value =
                serde_json::from_slice(&output.stdout).expect("signal must return one JSON object");
            assert_eq!(
                value["error"]["code"], "cancelled",
                "{command} phase {phase}: {value}"
            );
            assert!(!output.status.success());
            assert!(!out.exists());
            assert_eq!(
                fs::read(&sidecar).ok(),
                sidecar_before,
                "no settings write may start after render cancellation"
            );
            assert!(
                command == "print"
                    || !String::from_utf8_lossy(&output.stderr).contains("preparing"),
                "pre-test render cancellation must not start a print job"
            );
        }
    }
}

#[test]
fn check_device_argument_errors_are_json_before_bluetooth_or_lock() {
    for args in [
        vec!["check-device"],
        vec!["check-device", "--device", "malformed"],
        vec![
            "check-device",
            "--device",
            "malformed",
            "--model",
            "M110",
            "--copies",
            "1",
        ],
    ] {
        assert_eq!(cli(&args)["error"]["code"], "invalid_arguments");
    }
    assert_eq!(
        cli(&["check-device", "--device", "malformed", "--model", "M220"])["error"]["code"],
        "unsupported_device"
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        cli(&["check-device", "--device", "malformed", "--model", "M110"])["error"]["code"],
        "invalid_arguments"
    );
}
