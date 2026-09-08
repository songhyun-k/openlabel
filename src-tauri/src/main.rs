mod commands;
use openlabel_core::{
    ipc::{Endpoint, Host},
    job::Jobs,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::Manager;
fn request_exit(app: &tauri::AppHandle) {
    let jobs = app.state::<Arc<Jobs>>().inner().clone();
    let host = app.state::<Arc<Host>>().inner().clone();
    host.stop_admission();
    if !jobs.begin_close() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        jobs.shutdown().await;
        host.stopped().await;
        app.state::<AtomicBool>().store(true, Ordering::SeqCst);
        app.exit(0);
    });
}
fn main() {
    if openlabel_core::label::render_worker() {
        return;
    }
    let jobs = Arc::new(Jobs::persistent());
    let host = tauri::async_runtime::block_on(async {
        Host::start(&Endpoint::current()?, jobs.clone()).await
    })
    .expect("Could not start the OpenLabel local shared connection");
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(jobs)
        .manage(host)
        .manage(AtomicBool::new(false))
        .invoke_handler(tauri::generate_handler![
            commands::set_ui_locale,
            commands::preview_label,
            commands::save_settings,
            commands::scan_devices,
            commands::start_print,
            commands::get_job,
            commands::cancel_job,
            commands::get_runtime,
            commands::connect_device,
            commands::disconnect_device
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                request_exit(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("Failed to run OpenLabel");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event
            && !app.state::<AtomicBool>().load(Ordering::SeqCst)
        {
            api.prevent_exit();
            request_exit(app);
        }
    });
}
