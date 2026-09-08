#[path = "../src/commands.rs"]
mod commands;
use openlabel_core::job::Jobs;
use serde_json::json;
use std::sync::Arc;
#[cfg(unix)]
#[test]
fn ipc_bootstrap_uses_tauri_runtime_without_an_ambient_tokio_reactor() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = openlabel_core::ipc::Endpoint {
        directory: directory.path().join("ipc"),
    };
    tauri::async_runtime::block_on(async {
        let jobs = Arc::new(Jobs::persistent());
        let host = openlabel_core::ipc::Host::start(&endpoint, jobs.clone())
            .await
            .unwrap();
        host.stop_admission();
        jobs.shutdown().await;
        host.stopped().await;
    });
}
#[test]
fn tauri_commands_async_start_admission_and_cancel_without_native_window() {
    let jobs = Arc::new(Jobs::default());
    let app = tauri::test::mock_builder()
        .manage(jobs.clone())
        .invoke_handler(tauri::generate_handler![
            commands::set_ui_locale,
            commands::start_print,
            commands::get_job,
            commands::cancel_job,
            commands::preview_label,
            commands::save_settings,
            commands::scan_devices,
            commands::get_runtime,
            commands::connect_device,
            commands::disconnect_device
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let error = commands::set_ui_locale(app.handle().clone(), "unsupported".into()).unwrap_err();
    assert_eq!(error.code, "invalid_arguments");
    assert_eq!(error.message.unwrap().key, "err.uiLocale");
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let response = tauri::test::get_ipc_response(
        &window,
        tauri::webview::InvokeRequest {
            cmd: "start_print".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(
                json!({"request":{"label":{},"device":"test-never-connect","model":"M110","copies":1,"expect_sha256":"0".repeat(64),"expect_input_sha256":"0".repeat(64)}}),
            ),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    );
    let value = response
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap();
    assert_eq!(value["state"], "preparing");
    let id = value["id"].as_u64().unwrap();
    jobs.cancel(id);
    for _ in 0..120 {
        if jobs.status().unwrap().finished {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("cancelled command did not finish bounded cleanup");
}
