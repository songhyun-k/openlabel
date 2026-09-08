import { systemLocale, translate, type Locale } from "./i18n";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { ConnectionStatus, Device, Job, LabelRequest, Preview, PrintRequest, Runtime, Settings } from "./contracts";

const localeKey = "openlabel.locale.v1";
export function readLocale(): Locale {
  try {
    const saved = localStorage.getItem(localeKey);
    if (saved === "en" || saved === "ko") return saved;
  } catch { /* System language remains available without storage. */ }
  return systemLocale(navigator.language);
}
export function writeLocale(locale: Locale): boolean {
  try { localStorage.setItem(localeKey, locale); return true; } catch { return false; }
}

// Serialize display updates so a slower earlier selection cannot win the race.
let menuUpdate: Promise<void> = Promise.resolve();
export function setUiLocale(locale: Locale): Promise<void> {
  menuUpdate = menuUpdate.catch(() => {}).then(() => invoke<void>("set_ui_locale", { locale }));
  return menuUpdate;
}

const paperKey = "openlabel.paper.v1";
function validPaper(value: unknown): value is Settings["paper"] {
  if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).length !== 2) return false;
  const { width_mm, height_mm } = value as Settings["paper"];
  return typeof width_mm === "number" && Number.isFinite(width_mm) && width_mm >= 20 && width_mm <= 50
    && typeof height_mm === "number" && Number.isFinite(height_mm) && height_mm >= 10 && height_mm <= 100;
}
export function readPaper(): { paper?: Settings["paper"]; unavailable?: boolean } {
  let raw: string | null;
  try { raw = localStorage.getItem(paperKey); } catch { return { unavailable: true }; }
  if (!raw || raw.length > 256) return {};
  try { const paper: unknown = JSON.parse(raw); return validPaper(paper) ? { paper } : {}; } catch { return {}; }
}
export function writePaper(paper: Partial<Settings["paper"]>): boolean | null {
  if (!validPaper(paper)) return null;
  try { localStorage.setItem(paperKey, JSON.stringify(paper)); return true; } catch { return false; }
}

export const previewLabel = (request: LabelRequest) => invoke<Preview>("preview_label", { request });
export const scanDevices = () => invoke<Device[]>("scan_devices");
export const startPrint = (request: PrintRequest) => invoke<Job>("start_print", { request });
export const getRuntime = () => invoke<Runtime>("get_runtime");
export const connectDevice = (device: string) => invoke<ConnectionStatus>("connect_device", { device, model: "M110" });
export const disconnectDevice = (device: string) => invoke<ConnectionStatus>("disconnect_device", { device });
export const cancelJob = (id: number) => invoke<void>("cancel_job", { id });
export const saveSettingsFile = (request: LabelRequest, path: string, expectInputSha256: string) =>
  invoke<Preview>("save_settings", { request, path, overwrite: true, expectInputSha256 });
export const chooseLabel = (locale: Locale) => open({ title: translate(locale, "openImage"), multiple: false, filters: [{ name: translate(locale, "dialogImages"), extensions: ["svg", "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"] }] });
export const chooseSettings = (locale: Locale) => open({ title: translate(locale, "loadSettings"), multiple: false, filters: [{ name: translate(locale, "dialogSettings"), extensions: ["json"] }] });
export const chooseSettingsTarget = (path: string, locale: Locale) => save({ title: translate(locale, "saveSettings"), defaultPath: /[^/\\]\.svg$/i.test(path) ? path.replace(/\.svg$/i, ".openlabel.json") : `${path}.openlabel-image.json`, filters: [{ name: translate(locale, "dialogSettings"), extensions: ["json"] }] });
export const subscribeDrop = (receive: (path: string) => void) =>
  getCurrentWebview().onDragDropEvent(({ payload }) => {
    if (payload.type === "drop" && payload.paths.length === 1) receive(payload.paths[0]);
  });
