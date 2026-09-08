import { useEffect, useRef, useState } from "react";
import type { ConnectionStatus, Device, Job, LabelRequest, Overrides, Preview, Settings } from "./contracts";
import * as native from "./native";

import { msg, errorNotice, type Notice, type MessageKey } from "./i18n";
const stateKeys: Record<string, MessageKey> = { preparing: "preparing", sending: "sending", completed: "completed", cancelled: "cancelled", failed: "failed" };
const jobNotice = (job: Job) => msg(stateKeys[job.state] ?? "unknownState", { state: job.state });
const terminalNotice = (job: Job) => job.error ? errorNotice(job.error) : jobNotice(job);
const terminal = (job: Job | null) => !!job && ["completed", "cancelled", "failed"].includes(job.state);
const invalidatesSource = (job: Job) => !!job.error && ["hash_mismatch", "settings_source_mismatch", "invalid_label", "invalid_settings"].includes(job.error.code);
export const numberFields = {
  scale_percent: ["scale", 10, 200, 0.1],
  width_mm: ["paperWidth", 20, 50, 0.125], height_mm: ["paperHeight", 10, 100, 0.125],
  margin_mm: ["margin", 0, 5, 0.125], offset_x_mm: ["offsetX", -10, 10, 0.125], offset_y_mm: ["offsetY", -10, 10, 0.125],
  threshold: ["threshold", 0, 255, 1], white_cutoff: ["whiteCutoff", 0, 255, 1], density: ["density", 1, 15, 1], speed: ["speed", 1, 5, 1],
} as const;
function flatten(s: Settings): Overrides {
  return { ...s.paper, ...s.layout, scale_percent: s.layout.scale_percent ?? 100, raster_mode: s.raster.mode, threshold: s.raster.threshold, white_cutoff: s.raster.white_cutoff, ...s.printer };
}

export function usePrintWorkflow() {
  const [locale, setLocale] = useState(native.readLocale);
  const [localeStorageFailed, setLocaleStorageFailed] = useState(false);
  function selectLocale(value: string) {
    if (value !== "en" && value !== "ko") return;
    setLocale(value);
    setLocaleStorageFailed(!native.writeLocale(value));
  }
  const [menuError, setMenuError] = useState<Notice | null>(null);
  useEffect(() => {
    let alive = true;
    setMenuError(null);
    void native.setUiLocale(locale).catch(error => {
      if (alive) setMenuError(msg("menuUpdateFailed", { error: errorNotice(error) }));
    });
    return () => { alive = false; };
  }, [locale]);
  const [rememberedPaper] = useState(native.readPaper);
  const paper = useRef<Partial<Settings["paper"]>>(rememberedPaper.paper ?? {});
  const [paperStorageFailed, setPaperStorageFailed] = useState(!!rememberedPaper.unavailable);
  const [path, setPath] = useState<string | null>(null), [settingsPath, setSettingsPath] = useState<string | null>(null);
  const [defaults, setDefaults] = useState(false), [pattern, setPattern] = useState(false), [overrides, setOverrides] = useState<Overrides>(paper.current);
  const [preview, setPreview] = useState<Preview | null>(null), [settings, setSettings] = useState<Settings | null>(null);
  const [previewStatus, setPreviewStatus] = useState<Notice | null>(null), [previewError, setPreviewError] = useState<Notice | null>(null);
  const [discoveryStatus, setDiscoveryStatus] = useState<Notice | null>(null), [fileStatus, setFileStatus] = useState<Notice | null>(null), [jobStatus, setJobStatus] = useState<Notice | null>(null);
  const [devices, setDevices] = useState<Device[]>([]), [device, setDevice] = useState(""), [attested, setAttested] = useState(false), [copies, setCopies] = useState(1);
  const [job, setJob] = useState<Job | null>(null), [working, setWorking] = useState(false), [scanning, setScanning] = useState(false), [revision, setRevision] = useState(0);
  const generation = useRef(0), action = useRef(0), printing = useRef(false), blocked = useRef(false), mounted = useRef(true);
  const activeJob = useRef<Job | null>(null);
  const [connection, setConnection] = useState<ConnectionStatus>({ state: "disconnected", device: null, evidence: null, error: null });
  const [connectionError, setConnectionError] = useState<Notice | null>(null), [runtimeBusy, setRuntimeBusy] = useState(false);
  const [runtimeKnown, setRuntimeKnown] = useState(false);
  const connectionObservation = useRef(0);
  const ownJob = useRef<number | null>(null), pendingOwn = useRef<number | null>(null), ownFinal = useRef<number | null>(null);
  const ownAfter = useRef(0);
  const ownFinished = useRef<number | null>(null);
  const latestJob = useRef<Job | null>(null), dismissed = useRef<number | null>(null), adopted = useRef<number | null>(null);
  const locked = working || runtimeBusy || (!!job && !job.finished);
  blocked.current = locked || scanning;
  const effectiveScale = pattern ? 100 : overrides.scale_percent ?? settings?.layout.scale_percent ?? 100;
  const request: LabelRequest = { path: pattern ? null : path, test_pattern: pattern, settings: pattern ? null : settingsPath, defaults: pattern ? false : defaults, overrides: pattern ? { ...overrides, scale_percent: 100 } : overrides, snapshot: null };
  const requestKey = JSON.stringify(request);
  const invalidNumber = Object.entries(numberFields).find(([key, [, min, max, step]]) => {
    const value = key === "scale_percent" && pattern ? 100 : overrides[key as keyof typeof numberFields];
    return value !== undefined && (!Number.isFinite(value) || value < min || value > max || (step === 1 && !Number.isInteger(value)));
  });
  const numericError = invalidNumber
    ? msg("numericInvalid", { field: msg(invalidNumber[1][0]), min: invalidNumber[1][1], max: invalidNumber[1][2], kind: msg(invalidNumber[1][3] === 1 ? "integer" : "finiteNumber") })
    : !pattern && (!Number.isInteger(copies) || copies < 1 || copies > 10) ? msg("copiesInvalid") : null;
  const numericErrorKey = JSON.stringify(numericError);
  function begin() {
    action.current++;
    if (latestJob.current?.finished) { dismissed.current = latestJob.current.id; setJob(null); }
    setJobStatus(null); setDiscoveryStatus(null); setFileStatus(null);
    return action.current;
  }
  const current = (token: number) => mounted.current && action.current === token;
  function invalidate() { generation.current++; setPreview(null); setPreviewStatus(null); }
  function rememberPaper() {
    const saved = native.writePaper(paper.current);
    if (saved !== null) setPaperStorageFailed(!saved);
  }
  function refresh() {
    if (printing.current || blocked.current) return;
    begin(); invalidate(); setRevision(n => n + 1);
  }
  function openLabel(file: string) {
    if (!mounted.current || printing.current || blocked.current) return;
    begin(); invalidate(); setRevision(n => n + 1);
    setPath(file); setPattern(false); setSettingsPath(null); setDefaults(false); setOverrides({ ...paper.current }); setSettings(null);
    setPreviewError(null); setPreviewStatus(msg("previewCalculating"));
  }
  function change<K extends keyof Overrides>(key: K, value: Overrides[K]) {
    if (printing.current || blocked.current) return;
    const update = { [key]: value, ...(key === "raster_mode" && value === "floyd-steinberg" ? { threshold: 128 } : {}) };
    if (Object.entries(update).every(([field, next]) => Object.is(overrides[field as keyof Overrides], next))) return;
    if (key === "width_mm" || key === "height_mm") { paper.current = { ...paper.current, [key]: value }; rememberPaper(); }
    begin(); invalidate(); setOverrides(o => ({ ...o, ...update }));
  }
  function selectDevice(value: string) {
    if (printing.current || blocked.current || (connection.device && value !== connection.device)) return;
    begin(); setDevice(value); setAttested(false);
  }
  function attest(value: boolean) { if (!printing.current && !blocked.current) { begin(); setAttested(value); } }
  function changeCopies(value: number) { if (!printing.current && !blocked.current) { begin(); setCopies(value); } }
  useEffect(() => {
    mounted.current = true;
    let alive = true, unlisten: (() => void) | undefined;
    const token = action.current;
    void native.subscribeDrop(file => { if (alive) openLabel(file); }).then(fn => { if (alive) unlisten = fn; else fn(); }).catch(e => {
      if (alive && current(token)) setFileStatus(errorNotice(e));
    });
    return () => { alive = false; mounted.current = false; unlisten?.(); };
  }, []);
  useEffect(() => {
    const token = ++generation.current;
    setPreview(null); setPreviewError(null);
    if (!path && !pattern) { setPreviewStatus(null); return; }
    if (numericError) { setPreviewStatus(null); return; }
    let stopped = false, timer: ReturnType<typeof setTimeout>;
    setPreviewStatus(msg("previewCalculating"));
    const render = async () => {
      try {
        const result = await native.previewLabel(JSON.parse(requestKey) as LabelRequest);
        if (!stopped && generation.current === token) {
          if (!pattern) { paper.current = { ...result.settings.paper }; rememberPaper(); }
          setPreview(result); setSettings(result.settings);
          setPreviewStatus(null);
        }
      } catch (e) {
        if (!stopped && generation.current === token) {
          if (typeof e === "object" && e !== null && "code" in e && e.code === "render_busy") timer = setTimeout(render, 200);
          else { setPreviewStatus(null); setPreviewError(errorNotice(e)); }
        }
      }
    };
    timer = setTimeout(render, 150);
    return () => { stopped = true; clearTimeout(timer); };
  }, [requestKey, revision, numericErrorKey]);
  function observeOwn(result: Job) {
    ownJob.current = result.id;
    if (terminal(result) && ownFinal.current !== result.id) {
      ownFinal.current = result.id;
      if (invalidatesSource(result)) invalidate();
    }
    if (result.finished && ownFinished.current !== result.id) { ownFinished.current = result.id; printing.current = false; setWorking(false); }
  }
  useEffect(() => {
    let stopped = false, timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const runtime = await native.getRuntime();
        if (!stopped) {
          connectionObservation.current++;
          const nativeBusy = runtime.busy || runtime.closing || ["connecting", "disconnecting"].includes(runtime.connection.state);
          setConnection(runtime.connection); setConnectionError(null); setRuntimeKnown(true); setRuntimeBusy(nativeBusy);
          if (nativeBusy || (runtime.job && !runtime.job.finished)) blocked.current = true;
          const own = runtime.desktop_job;
          if (own && (own.id === ownJob.current || (pendingOwn.current !== null && own.id > ownAfter.current))) observeOwn(own);
          const result = runtime.job;
          if (!result || result.id >= (latestJob.current?.id ?? ownJob.current ?? 0)) latestJob.current = result;
          if (result && result.id >= (ownJob.current ?? 0) && result.id >= (activeJob.current?.id ?? 0) && (!result.finished || result.id !== dismissed.current)) {
            if (result.origin === "cli" && adopted.current !== result.id) {
              adopted.current = result.id; action.current++;
            }
            activeJob.current = result; setJob(result);
            if (terminal(result)) setJobStatus(terminalNotice(result));
            else setJobStatus(result.origin === "cli" ? msg("cliStatus", { status: jobNotice(result) }) : null);
          }
        }
      } catch (e) { if (!stopped) { connectionObservation.current++; setConnectionError(errorNotice(e)); setConnection(previous => ({ ...previous, state: "unknown", evidence: null })); } }
      if (!stopped) timer = setTimeout(poll, 150);
    };
    timer = setTimeout(poll, 100);
    return () => { stopped = true; clearTimeout(timer); };
  }, []);
  async function pickLabel() {
    if (printing.current || blocked.current) return;
    const token = begin();
    try { const file = await native.chooseLabel(locale); if (current(token) && typeof file === "string") openLabel(file); }
    catch (e) { if (current(token)) setFileStatus(errorNotice(e)); }
  }
  async function loadSettings() {
    if (printing.current || blocked.current) return;
    const token = begin();
    try {
      const file = await native.chooseSettings(locale);
      if (current(token) && typeof file === "string" && !printing.current && !blocked.current) {
        invalidate(); setSettingsPath(file); setDefaults(false); setOverrides({}); setRevision(n => n + 1);
      }
    } catch (e) { if (current(token)) setFileStatus(errorNotice(e)); }
  }
  async function saveSettings() {
    if (!preview || !path || printing.current || blocked.current) return;
    const token = begin(); blocked.current = true; setWorking(true);
    const snapshot = { ...request, snapshot: preview.settings };
    try {
      const target = await native.chooseSettingsTarget(path, locale);
      if (target && current(token)) {
        const result = await native.saveSettingsFile(snapshot, target, preview.input_sha256);
        if (current(token)) {
          invalidate(); setSettingsPath(target); setDefaults(false); setOverrides(flatten(result.settings)); setRevision(n => n + 1); setFileStatus(msg("settingsSaved"));
        }
      }
    } catch (e) { if (current(token)) setFileStatus(errorNotice(e)); }
    finally { if (mounted.current) setWorking(false); }
  }
  async function scan() {
    if (printing.current || blocked.current) return;
    const token = begin(); blocked.current = true; setScanning(true);
    setDiscoveryStatus(msg("discoverySearching"));
    try {
      const found = await native.scanDevices();
      if (current(token)) { setDevices(found); setDiscoveryStatus(found.length ? msg("discoveryChoose") : msg("discoveryEmpty")); }
    } catch (e) { if (current(token)) setDiscoveryStatus(errorNotice(e)); }
    finally { if (mounted.current) setScanning(false); }
  }
  const candidates = connection.evidence && !devices.some(d => d.id === connection.evidence?.device_id)
    ? [...devices, { id: connection.evidence.device_id, name: connection.evidence.model, rssi: null, supported: true, model_allowed: true, evidence: "connected", advertised_services: [connection.evidence.service] }] : devices;
  const targetAllowed = !!device && candidates.some(d => d.id === device && d.model_allowed) && (!connection.device || connection.device === device);
  const canConnect = targetAllowed && attested && !disabledLink();
  function disabledLink() { return locked || scanning || !runtimeKnown || !!connectionError; }
  async function link(disconnect: boolean) {
    if (disabledLink() || (disconnect ? !connection.device : !canConnect)) return;
    const token = begin(), observation = connectionObservation.current; setWorking(true); blocked.current = true;
    try { await (disconnect ? native.disconnectDevice(connection.device!) : native.connectDevice(device)); }
    catch (e) { if (current(token) && connectionObservation.current === observation) setConnectionError(errorNotice(e)); }
    finally { if (mounted.current) setWorking(false); }
  }
  const ready = !!preview && targetAllowed && attested && !locked && !scanning && !numericError && runtimeKnown && !connectionError;
  async function print() {
    if (!ready || printing.current || blocked.current || !preview) return;
    const token = begin(); printing.current = true; ownAfter.current = latestJob.current?.id ?? 0; pendingOwn.current = token; setWorking(true); setJobStatus(msg("revalidating"));
    try {
      const result = await native.startPrint({ label: { ...request, snapshot: preview.settings }, device, model: "M110", copies: pattern ? 1 : copies, expect_sha256: preview.sha256, expect_input_sha256: preview.input_sha256 });
      if (mounted.current && pendingOwn.current === token) observeOwn(result);
      if (current(token) && (!latestJob.current || latestJob.current.id <= result.id)) {
        const latest = latestJob.current;
        const snapshot = latest?.id === result.id ? latest : result;
        activeJob.current = snapshot;
        setJob(snapshot); setJobStatus(terminal(snapshot) ? terminalNotice(snapshot) : null);
        if (result.finished) { printing.current = false; setWorking(false); }
      }
    } catch (e) { if (mounted.current && pendingOwn.current === token) { printing.current = false; setWorking(false); } if (current(token)) { invalidate(); setJobStatus(errorNotice(e)); } }
    finally { if (pendingOwn.current === token) pendingOwn.current = null; }
  }
  async function cancel() {
    if (!job || job.finished || terminal(activeJob.current)) return;
    const token = action.current;
    setJobStatus(msg("cancelling"));
    try { await native.cancelJob(job.id); }
    catch (e) { if (current(token) && activeJob.current?.id === job.id && !activeJob.current.finished && !terminal(activeJob.current)) setJobStatus(errorNotice(e)); }
  }
  function togglePattern() {
    if (printing.current || blocked.current) return;
    begin(); invalidate();
    setOverrides(pending => {
      const next = { ...(settings ? flatten(settings) : {}), ...pending };
      delete next.width_mm; delete next.height_mm;
      return { ...next, ...paper.current, scale_percent: pattern || !settings ? pending.scale_percent : pending.scale_percent ?? settings.layout.scale_percent ?? 100 };
    });
    setPattern(!pattern);
  }
  function resetSettings() {
    if (printing.current || blocked.current) return;
    begin(); invalidate(); setDefaults(true); setSettingsPath(null); setOverrides({ ...paper.current }); setRevision(n => n + 1);
  }
  const hasLabel = !!path || pattern, disabled = locked || scanning;
  const rendering = previewStatus?.key === "previewCalculating";
  const refreshRequired = hasLabel && !preview && !rendering && !numericError && !previewError;
  const reason = numericError || (locked ? msg("waitCleanup") : scanning ? msg("searchingPrinter") : !hasLabel ? msg("chooseSource") : !preview ? msg("checkPreview") : !device ? msg("choosePrinter") : !targetAllowed ? msg("reselectModel") : !attested ? msg("confirmModel") : connectionError ? msg("checkConnection") : null);
  const cropNotice = preview && (effectiveScale > 100 || preview.clipped_dot_count > 0) ? msg("cropNotice") : null;
  const statusDetail = jobStatus || (job && !job.finished ? jobNotice(job) : null) || discoveryStatus || fileStatus || numericError || (previewError ? msg("previewRecovery", { error: previewError }) : null) || previewStatus || reason || cropNotice;
  const emptyHint = !hasLabel && statusDetail === reason && reason?.key === "chooseSource";
  const canCancel = !!job && !job.finished && !terminal(job);
  return { locale, menuError, localeStorageFailed, selectLocale, emptyHint, path, pattern, overrides, preview, settings, paperStorageFailed, statusDetail, rendering, refreshRequired, effectiveScale, previewError, devices: candidates, device, attested, copies, job, canCancel, locked, scanning, invalidNumber, numericError, ready, hasLabel, reason, disabled, connection, connectionError, canConnect, connect: () => link(false), disconnect: () => link(true), change, selectDevice, attest, changeCopies, pickLabel, loadSettings, saveSettings, scan, print, cancel, togglePattern, resetSettings, refresh };
}
