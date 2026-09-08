use openlabel_core::{
    Error, Result, ble,
    job::{ConnectionStatus, Job, Jobs, PrintRequest, Runtime},
    label::{self, Request},
    operation::Control,
    raster::Preview,
    settings,
};
use std::{path::PathBuf, sync::Arc};
#[tauri::command]
pub async fn preview_label(request: Request) -> Result<Preview> {
    label::preview_async(request).await
}
#[tauri::command]
pub async fn save_settings(
    request: Request,
    path: PathBuf,
    overwrite: bool,
    expect_input_sha256: String,
) -> Result<Preview> {
    if request.test_pattern {
        return Err(Error::localized(
            "invalid_settings",
            "err.saveFromImage",
            &[],
        ));
    }
    let preview = label::preview_async(request.clone()).await?;
    if preview.input_sha256 != expect_input_sha256 {
        return Err(Error::localized("hash_mismatch", "err.fileChanged", &[]));
    }
    settings::write_output(
        &path,
        &serde_json::to_vec_pretty(&preview.settings).unwrap(),
        overwrite,
        &request.path.iter().cloned().collect::<Vec<_>>(),
    )?;
    label::preview_async(Request {
        settings: Some(path),
        defaults: false,
        ..request
    })
    .await
}
#[tauri::command]
pub async fn scan_devices() -> Result<Vec<ble::Device>> {
    ble::devices(&Control::default()).await
}
#[tauri::command]
pub async fn start_print(request: PrintRequest, jobs: tauri::State<'_, Arc<Jobs>>) -> Result<Job> {
    jobs.inner().start(request)
}
#[tauri::command]
pub fn get_job(jobs: tauri::State<'_, Arc<Jobs>>) -> Option<Job> {
    jobs.status()
}
#[tauri::command]
pub fn cancel_job(id: u64, jobs: tauri::State<'_, Arc<Jobs>>) {
    jobs.cancel(id);
}
#[tauri::command]
pub fn get_runtime(jobs: tauri::State<'_, Arc<Jobs>>) -> Runtime {
    jobs.runtime()
}
#[tauri::command]
pub async fn connect_device(
    device: String,
    model: String,
    jobs: tauri::State<'_, Arc<Jobs>>,
) -> Result<ConnectionStatus> {
    jobs.link(device, Some(model), Control::default()).await
}
#[tauri::command]
pub async fn disconnect_device(
    device: String,
    jobs: tauri::State<'_, Arc<Jobs>>,
) -> Result<ConnectionStatus> {
    jobs.link(device, None, Control::default()).await
}

/// Display-only: preserve Tauri's default macOS roles and native keyboard actions.
#[tauri::command]
pub fn set_ui_locale<R: tauri::Runtime>(app: tauri::AppHandle<R>, locale: String) -> Result<()> {
    let catalog = match locale.as_str() {
        "en" => include_str!("../../src/locales/en.json"),
        "ko" => include_str!("../../src/locales/ko.json"),
        _ => return Err(Error::localized("invalid_arguments", "err.uiLocale", &[])),
    };
    #[cfg(target_os = "macos")]
    {
        use tauri::menu::{
            AboutMetadata, HELP_SUBMENU_ID, Menu, PredefinedMenuItem as Item, Submenu,
            WINDOW_SUBMENU_ID,
        };
        let texts: std::collections::BTreeMap<String, String> =
            serde_json::from_str(catalog).expect("valid menu message catalog");
        let menu_text = |key: &str| texts[key].as_str();
        let build = || -> tauri::Result<()> {
            let pkg = app.package_info();
            let about = AboutMetadata {
                name: Some(pkg.name.clone()),
                version: Some(pkg.version.to_string()),
                copyright: app.config().bundle.copyright.clone(),
                authors: app
                    .config()
                    .bundle
                    .publisher
                    .clone()
                    .map(|publisher| vec![publisher]),
                ..Default::default()
            };
            let menu = Menu::with_items(
                &app,
                &[
                    &Submenu::with_items(
                        &app,
                        "OpenLabel",
                        true,
                        &[
                            &Item::about(&app, Some(menu_text("menu.about")), Some(about))?,
                            &Item::separator(&app)?,
                            &Item::services(&app, Some(menu_text("menu.services")))?,
                            &Item::separator(&app)?,
                            &Item::hide(&app, Some(menu_text("menu.hide")))?,
                            &Item::hide_others(&app, Some(menu_text("menu.hideOthers")))?,
                            &Item::separator(&app)?,
                            &Item::quit(&app, Some(menu_text("menu.quit")))?,
                        ],
                    )?,
                    &Submenu::with_items(
                        &app,
                        menu_text("menu.file"),
                        true,
                        &[&Item::close_window(&app, Some(menu_text("menu.close")))?],
                    )?,
                    &Submenu::with_items(
                        &app,
                        menu_text("menu.edit"),
                        true,
                        &[
                            &Item::undo(&app, Some(menu_text("menu.undo")))?,
                            &Item::redo(&app, Some(menu_text("menu.redo")))?,
                            &Item::separator(&app)?,
                            &Item::cut(&app, Some(menu_text("menu.cut")))?,
                            &Item::copy(&app, Some(menu_text("menu.copy")))?,
                            &Item::paste(&app, Some(menu_text("menu.paste")))?,
                            &Item::select_all(&app, Some(menu_text("menu.selectAll")))?,
                        ],
                    )?,
                    &Submenu::with_items(
                        &app,
                        menu_text("menu.view"),
                        true,
                        &[&Item::fullscreen(&app, Some(menu_text("menu.fullscreen")))?],
                    )?,
                    &Submenu::with_id_and_items(
                        &app,
                        WINDOW_SUBMENU_ID,
                        menu_text("menu.window"),
                        true,
                        &[
                            &Item::minimize(&app, Some(menu_text("menu.minimize")))?,
                            &Item::maximize(&app, Some(menu_text("menu.zoom")))?,
                            &Item::separator(&app)?,
                            &Item::close_window(&app, Some(menu_text("menu.close")))?,
                        ],
                    )?,
                    &Submenu::with_id_and_items(
                        &app,
                        HELP_SUBMENU_ID,
                        menu_text("menu.help"),
                        true,
                        &[],
                    )?,
                ],
            )?;
            app.set_menu(menu)?;
            Ok(())
        };
        build().map_err(|error| Error::new("ui_locale_error", error.to_string()))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, catalog);
    Ok(())
}
