import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import {
  render,
  renderHook,
  screen,
  fireEvent,
  waitFor,
  cleanup,
  act,
} from "@testing-library/react";
import App from "./App";
import { usePrintWorkflow } from "./usePrintWorkflow";
import type { Job, LabelRequest, Preview, Runtime } from "./contracts";
import { readPaper, writePaper } from "./native";
const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  menu: vi.fn(),
  open: vi.fn(),
  save: vi.fn(),
  drop: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: async (command: string, args: unknown) => {
  if (command === "set_ui_locale") return mocks.menu(args);
  const result = await mocks.invoke(command, args);
  if (command !== "get_runtime" || result?.connection) return result;
  const job = result && "id" in result ? { origin: "desktop", ...result } : null;
  return { app_running: true, revision: 0, connection: { state: "disconnected", device: null, evidence: null, error: null }, job, desktop_job: job, busy: !!job && !job.finished, closing: false };
} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mocks.open,
  save: mocks.save,
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: mocks.drop,
  }),
}));
const fixture = (): Preview => ({
  sha256: "raster",
  input_sha256: "input",
  source_sha256: "source",
  settings_path: null,
  png_base64: "AA==",
  clipped_dot_count: 0,
  settings: {
    version: 1,
    source_sha256: "source",
    paper: { width_mm: 40, height_mm: 30 },
    layout: {
      alignment: "center",
      margin_mm: 1,
      rotation_deg: 0,
      mirror: false,
      offset_x_mm: 0,
      offset_y_mm: 0,
    },
    raster: { mode: "threshold", threshold: 128, white_cutoff: 255 },
    printer: { density: 10, speed: 1 },
  },
  geometry: {
    paper_x: 32,
    paper_width: 320,
    height: 240,
    head_width: 384,
    printable_x: 32,
    printable_width: 320,
    content_x: 40,
    content_y: 8,
    content_width: 304,
    content_height: 224,
  },
});
beforeEach(() => {
  vi.spyOn(navigator, "language", "get").mockReturnValue("ko-KR");
  vi.stubGlobal("CSS", { escape: (value: string) => Array.from(value, char => `\\${char.codePointAt(0)!.toString(16)} `).join("") });
  const storage = new Map<string, string>([["openlabel.locale.v1", "ko"]]);
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), removeItem: (key: string) => storage.delete(key) });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  mocks.invoke.mockReset();
  mocks.menu.mockReset();
  mocks.menu.mockResolvedValue(undefined);
  mocks.open.mockReset();
  mocks.save.mockReset();
  mocks.drop.mockReset();
  mocks.drop.mockResolvedValue(() => {});
  mocks.open.mockResolvedValue("/label.svg");
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "preview_label") return Promise.resolve(fixture());
    if (command === "scan_devices")
      return Promise.resolve([
        {
          id: "exact-id",
          name: "M110",
          rssi: -50,
          supported: false,
          model_allowed: true,
          evidence: "candidate",
        },
      ]);
    return Promise.resolve(null);
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
async function openAndSelect() {
  fireEvent.click(screen.getByText("이미지 열기"));
  await screen.findByAltText(/최종 흑백/);
  reveal(screen.getByText("Bluetooth 검색"));
  fireEvent.click(screen.getByText("Bluetooth 검색"));
  await choose("장치 선택", "exact-id");
  fireEvent.click(screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인"));
}
const choice = (label: string) => document.querySelector<HTMLButtonElement>(`.select__trigger[aria-label="${label}"]`)!;
function reveal(control: HTMLElement) {
  const root = control.closest(".accordion");
  const trigger = root?.querySelector<HTMLButtonElement>(".accordion__trigger");
  if (trigger?.getAttribute("aria-expanded") === "false") fireEvent.click(trigger);
}
async function choices(label: string) {
  const trigger = choice(label); reveal(trigger);
  await waitFor(() => expect(trigger.disabled).toBe(false));
  act(() => trigger.focus());
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  return screen.findAllByRole("option");
}
async function choose(label: string, key: string) {
  const options = screen.queryAllByRole("option").length ? screen.getAllByRole("option") : await choices(label);
  const option = options.find(option => option.getAttribute("data-key") === key);
  expect(option, `${label}: ${key}`).toBeTruthy();
  fireEvent.click(option!);
  await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(choice(label)));
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function expectMutationsLocked() {
  const headView = screen.getByLabelText("head 전체 보기");
  const controls = [...document.querySelectorAll<HTMLInputElement | HTMLButtonElement>(".inspector-scroll input, .inspector-scroll button, .print-action input, .page-header button")].filter(control => control !== headView && !control.closest(".language-control") && !control.classList.contains("accordion__trigger"));
  expect(controls.length).toBeGreaterThan(20);
  for (const control of controls) expect(control.disabled).toBe(true);
  for (const trigger of document.querySelectorAll<HTMLButtonElement>(".accordion__trigger")) expect(trigger.disabled).toBe(false);
  expect(choice("화면 확대").disabled).toBe(false);
  expect(choice("언어").disabled).toBe(false);
  expect((screen.getByLabelText("head 전체 보기") as HTMLInputElement).disabled).toBe(false);
  for (const name of ["이미지 열기", "이미지 맞춤", "보정 패턴", "미리보기 새로고침"]) expect((screen.getByText(name) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByLabelText("이미지 크기 (%)") as HTMLInputElement).disabled).toBe(true);
  const copies = screen.getByLabelText("인쇄 매수") as HTMLInputElement;
  expect(copies.closest(".print-row")).toBeTruthy();
  expect(copies.disabled).toBe(true);
}
const runtimeFixture = (): Runtime => ({ app_running: true, revision: 0, connection: { state: "disconnected", device: null, evidence: null, error: null }, job: null, desktop_job: null, busy: false, closing: false });
const jobFixture = (id: number, origin: string, state: string, finished: boolean): Job => ({ id, origin, state, finished, sent_bytes: 0, total_bytes: 10, copies: 1, device: "exact-id", sha256: "raster", input_sha256: "input", settings: null, evidence: null, error: null });
function mediaPreview(request: LabelRequest): Preview {
  const p = fixture(), o = request.overrides;
  const adjacent = request.path === "/with-sidecar.png" || request.path === "/first-sidecar.png";
  const initial = request.test_pattern ? [40, 30] : request.settings ? [30, 60] : request.path === "/first.svg" || request.path === "/first-sidecar.png" ? [50, 80] : adjacent ? [30, 60] : [50, 30];
  p.settings.paper = { width_mm: o.width_mm ?? initial[0], height_mm: o.height_mm ?? initial[1] };
  p.settings.source_sha256 = p.source_sha256 = request.path ?? "pattern";
  p.settings_path = request.settings ?? (adjacent ? `${request.path}.openlabel-image.json` : null);
  if (adjacent) p.settings.layout.rotation_deg = 180;
  for (const key of ["scale_percent", "rotation_deg", "mirror", "offset_x_mm", "offset_y_mm"] as const) {
    if (o[key] !== undefined) Object.assign(p.settings.layout, { [key]: o[key] });
  }
  p.sha256 = JSON.stringify(p.settings); p.input_sha256 = `${request.path}:${request.settings}`;
  p.geometry.paper_width = p.settings.paper.width_mm * 8; p.geometry.height = p.settings.paper.height_mm * 8;
  return p;
}
function mockMediaPreview() {
  const base = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) => command === "preview_label" ? Promise.resolve(mediaPreview(args.request)) : base(command, args));
}
const latestPreviewRequest = () => mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").at(-1)![1].request as LabelRequest;
const storedPaper = () => JSON.parse(localStorage.getItem("openlabel.paper.v1") ?? "null");
describe("dark HeroUI workspace", () => {
  it("uses keyboard popup selection, Escape and focus restoration without preview changes", async () => {
    render(<App />); await openAndSelect();
    const count = mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").length;
    await choices("화면 확대");
    expect(screen.getByRole("listbox").closest(".select__popover")).toBeTruthy();
    fireEvent.keyDown(screen.getByRole("listbox"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(choice("화면 확대")));
    await choose("화면 확대", "2");
    expect(choice("화면 확대").textContent).toContain("2배 도트");
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label")).toHaveLength(count);
  });
  it("rejects unavailable candidates and mutations when a job locks an open menu", async () => {
    const runtime = runtimeFixture(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "scan_devices" ? Promise.resolve([{ id: "exact-id", name: "M110", model_allowed: true }, { id: "unavailable", name: "Other", model_allowed: false }]) : base(command, args));
    render(<App />); await openAndSelect();
    await choices("장치 선택");
    const unavailable = screen.getByRole("option", { name: /unavailable/ });
    expect(unavailable.getAttribute("aria-disabled")).toBe("true"); fireEvent.click(unavailable);
    expect(choice("장치 선택").textContent).toContain("exact-id");
    fireEvent.keyDown(screen.getByRole("listbox"), { key: "Escape" });
    await choices("회전"); const quarter = screen.getByRole("option", { name: "90°" });
    const count = mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").length;
    runtime.job = jobFixture(9, "cli", "sending", false); runtime.busy = true;
    await waitFor(() => expect(choice("회전").disabled).toBe(true));
    fireEvent.click(quarter);
    expect(choice("회전").textContent).toContain("0°");
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label")).toHaveLength(count);
    expect(mocks.invoke.mock.calls.some(c => c[0] === "start_print")).toBe(false);
  });
  it("keeps collapsed panels mounted but inaccessible and retains inputs across independent toggles", async () => {
    render(<App />); await openAndSelect();
    const advanced = screen.getByRole("button", { name: "고급 설정" });
    const density = screen.getByLabelText("농도") as HTMLInputElement;
    expect(screen.queryByRole("spinbutton", { name: "농도" })).toBeNull();
    expect(density.closest(".accordion__panel")).toBeTruthy();
    expect(choice("회전").closest(".accordion")).toBeNull();
    expect(screen.getByRole("checkbox", { name: "좌우 반전" })).toBeTruthy();
    fireEvent.click(advanced); fireEvent.change(density, { target: { value: "12" } });
    fireEvent.click(advanced); expect(advanced.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(screen.getByRole("button", { name: "상세 정보" }));
    expect(advanced.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(advanced); expect(screen.getByRole("spinbutton", { name: "농도" })).toBe(density); expect(density.value).toBe("12");
  });
  it("declares dark root/portals, neutral selection, white paper and reduced motion", async () => {
    const { readFileSync } = await vi.importActual<{ readFileSync: (path: string, encoding: string) => string }>("node:fs");
    const styles = readFileSync("src/styles.css", "utf8"), tokens = readFileSync("src/design-tokens.css", "utf8");
    expect(readFileSync("index.html", "utf8")).toContain('class="dark" data-theme="dark"');
    expect(tokens).toContain("--background: #010102"); expect(tokens).toContain("--paper: #ffffff");
    expect(styles).toMatch(/\.paper \{[^}]*var\(--paper\)/);
    expect(styles).toMatch(/\.select__popover, \.tooltip \{[^}]*background: var\(--overlay\)/);
    expect(styles).toMatch(/\.button--secondary \{ --button-fg: var\(--foreground\); border: 1px solid var\(--separator\); \}/);
    expect(styles).toContain('.input:hover:not(:disabled):not([aria-invalid="true"])');
    expect(styles).toMatch(/\.checkbox \{[^}]*--accent: var\(--foreground\); --accent-hover: var\(--secondary\); --accent-foreground: var\(--background\)/);
    expect(styles).toMatch(/\.list-box-item \{[^}]*border-radius: var\(--radius\)/);
    expect(styles).toContain(':is(button, input:not([type="checkbox"]), [role="option"]):is(:focus-visible, [data-focus-visible="true"]), .checkbox__content[data-focus-visible="true"] .checkbox__control { outline: 3px solid var(--focus); outline-offset: 3px; }');
    expect(styles).not.toContain(':is(button, input):focus-visible, [data-focus-visible="true"]');
    for (const [token, duration] of [["fast",150],["panel",250],["exit",100]]) expect(tokens).toContain(`--motion-${token}: ${duration}ms`);
    expect(styles).toContain("animation: none !important; transition: none !important;");
  });
});
describe("artwork rotation and remembered paper", () => {
  it("preserves paper across completed print, pending edits, picker/drop images and remount", async () => {
    mockMediaPreview();
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "start_print" ? Promise.resolve(jobFixture(1, "desktop", "completed", true)) : base(command, args));
    render(<App />); await openAndSelect();
    fireEvent.change(screen.getByLabelText("용지 높이 (mm)"), { target: { value: "80" } });
    await screen.findByAltText(/50 × 80mm/);
    fireEvent.click(screen.getByText("1장 인쇄")); await screen.findByText("인쇄 완료");
    await choose("회전", "90");
    mocks.open.mockResolvedValue("/other.png"); fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByAltText(/50 × 80mm/);
    expect(latestPreviewRequest()).toMatchObject({ path: "/other.png", overrides: { width_mm: 50, height_mm: 80 } });
    expect(latestPreviewRequest().overrides.rotation_deg).toBeUndefined();
    // The registered callback was created on mount, before either media edit.
    fireEvent.change(screen.getByLabelText("용지 높이 (mm)"), { target: { value: "70" } });
    act(() => mocks.drop.mock.calls[0][0]({ payload: { type: "drop", paths: ["/drop.svg"] } }));
    await screen.findByAltText(/50 × 70mm/); expect(latestPreviewRequest().path).toBe("/drop.svg");
    fireEvent.change(screen.getByLabelText("용지 높이 (mm)"), { target: { value: "80" } });
    mocks.open.mockResolvedValue("/with-sidecar.png"); fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByAltText(/50 × 80mm/); expect(storedPaper()).toEqual({ width_mm: 50, height_mm: 80 });
    expect(choice("회전").textContent).toContain("180°");
    expect(screen.getByText("설정: /with-sidecar.png.openlabel-image.json")).toBeTruthy();
    cleanup(); render(<App />);
    expect((screen.getByLabelText("용지 높이 (mm)") as HTMLInputElement).value).toBe("80");
    fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/50 × 80mm/);
  });
  it("first successful source paper and explicit settings load establish both axes; Reset and Fit keep them", async () => {
    mockMediaPreview(); mocks.open.mockResolvedValue("/first-sidecar.png");
    render(<App />); fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/50 × 80mm/);
    expect(latestPreviewRequest().overrides).toEqual({});
    mocks.open.mockResolvedValue("/next.png"); fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/50 × 80mm/);
    mocks.open.mockResolvedValue("/custom.json"); fireEvent.click(screen.getByText("설정 불러오기")); await screen.findByAltText(/30 × 60mm/);
    expect(latestPreviewRequest().overrides).toEqual({}); expect(storedPaper()).toEqual({ width_mm: 30, height_mm: 60 });
    fireEvent.change(screen.getByLabelText("이미지 크기 (%)"), { target: { value: "150" } });
    fireEvent.click(screen.getByText("이미지 맞춤")); await screen.findByAltText(/30 × 60mm/);
    fireEvent.click(screen.getByText("설정 초기화")); await screen.findByAltText(/30 × 60mm/);
    expect(latestPreviewRequest()).toMatchObject({ defaults: true, settings: null, overrides: { width_mm: 30, height_mm: 60 } });
  });
  it.each(["", "51"])("keeps invalid paper %s across Reset/new image without preview or persistence", async value => {
    mockMediaPreview(); render(<App />); await openAndSelect();
    const before = mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").length, saved = storedPaper();
    fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), { target: { value } });
    fireEvent.click(screen.getByText("설정 초기화"));
    mocks.open.mockResolvedValue("/new.png"); fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(screen.getByText("new.png")).toBeTruthy());
    expect((screen.getByLabelText("용지 폭 (mm)") as HTMLInputElement).value).toBe(value);
    expect(screen.getByRole("status").textContent).toContain("숫자 입력값");
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 180)); });
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label")).toHaveLength(before); expect(storedPaper()).toEqual(saved);
  });
  it.each([false, true])("pattern cannot establish paper after pending/failed initial preview, failed=%s", async failed => {
    const pending = deferred<Preview>(); let ordinary = 0;
    mockMediaPreview(); const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "preview_label" && !args.request.test_pattern && ++ordinary === 1 ? pending.promise : base(command, args));
    mocks.open.mockResolvedValue("/first.svg"); render(<App />); fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(ordinary).toBe(1));
    if (failed) await act(async () => pending.reject({ detail: "first source failed" }));
    fireEvent.click(screen.getByText("보정 패턴")); await screen.findByAltText(/40 × 30mm/);
    expect(storedPaper()).toBeNull(); expect(latestPreviewRequest().overrides.width_mm).toBeUndefined();
    fireEvent.click(screen.getByText("라벨 보기")); await screen.findByAltText(/50 × 80mm/);
    expect(latestPreviewRequest().overrides.width_mm).toBeUndefined(); expect(latestPreviewRequest().overrides.height_mm).toBeUndefined();
    expect(storedPaper()).toEqual({ width_mm: 50, height_mm: 80 });
    if (!failed) await act(async () => pending.resolve(fixture()));
    fireEvent.click(screen.getByText("미리보기 새로고침")); await screen.findByAltText(/50 × 80mm/);
    expect(storedPaper()).toEqual({ width_mm: 50, height_mm: 80 });
  });
  it("pattern only retains explicitly edited media axes before an ordinary result", async () => {
    mockMediaPreview(); render(<App />);
    fireEvent.click(screen.getByText("보정 패턴")); await screen.findByAltText(/40 × 30mm/);
    fireEvent.change(screen.getByLabelText("용지 높이 (mm)"), { target: { value: "80" } });
    await screen.findByAltText(/40 × 80mm/); expect(storedPaper()).toBeNull();
    mocks.open.mockResolvedValue("/next.png"); fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/50 × 80mm/);
    expect(latestPreviewRequest().overrides).toEqual({ height_mm: 80 }); expect(storedPaper()).toEqual({ width_mm: 50, height_mm: 80 });
    fireEvent.click(screen.getByText("보정 패턴")); await screen.findByAltText(/50 × 80mm/);
    fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), { target: { value: "45" } });
    fireEvent.click(screen.getByText("라벨 보기")); await screen.findByAltText(/45 × 80mm/);
    expect(storedPaper()).toEqual({ width_mm: 45, height_mm: 80 });
  });
  it("stale preview and settings dialog cannot regress current media or storage", async () => {
    const stale = deferred<Preview>(), dialog = deferred<string>();
    mockMediaPreview(); const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "preview_label" && args.request.path === "/old.svg" ? stale.promise : base(command, args));
    render(<App />); mocks.open.mockResolvedValue("/old.svg"); fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(mocks.invoke.mock.calls.some(c => c[0] === "preview_label")).toBe(true));
    mocks.open.mockResolvedValue("/first.svg"); fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/50 × 80mm/);
    await act(async () => stale.resolve(fixture())); expect(storedPaper()).toEqual({ width_mm: 50, height_mm: 80 });
    mocks.open.mockReturnValue(dialog.promise); fireEvent.click(screen.getByText("설정 불러오기"));
    fireEvent.change(screen.getByLabelText("용지 높이 (mm)"), { target: { value: "70" } });
    await act(async () => dialog.resolve("/stale.json")); await screen.findByAltText(/50 × 70mm/);
    expect(latestPreviewRequest().settings).toBeNull(); expect(storedPaper()).toEqual({ width_mm: 50, height_mm: 70 });
  });
  it("offers all clockwise rotations, invalidates each preview and prints its exact snapshot/two hashes", async () => {
    mockMediaPreview(); const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "start_print" ? Promise.resolve(jobFixture(1, "desktop", "completed", true)) : base(command, args));
    render(<App />); await openAndSelect();
    expect((await choices("회전")).map(o => o.getAttribute("data-key"))).toEqual(["0", "90", "180", "270"]);
    for (const angle of [90, 180, 270, 0, 270]) {
      await choose("회전", String(angle));
      expect(screen.queryByAltText(/최종 흑백/)).toBeNull(); expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
      await screen.findByAltText(/최종 흑백/); expect(latestPreviewRequest().overrides.rotation_deg).toBe(angle);
    }
    const reviewed = mediaPreview(latestPreviewRequest()); fireEvent.click(screen.getByText("1장 인쇄"));
    expect(mocks.invoke.mock.calls.find(c => c[0] === "start_print")![1].request).toMatchObject({ expect_sha256: reviewed.sha256, expect_input_sha256: reviewed.input_sha256, label: { snapshot: reviewed.settings } });
  });
  it("rejects malformed storage and invalid writes using only the bounded paper pair", () => {
    for (const raw of ["{", "null", "[]", "x".repeat(257), '{"width_mm":50}', '{"width_mm":"50","height_mm":80}', '{"width_mm":51,"height_mm":80}', '{"width_mm":50,"height_mm":0}', '{"width_mm":1e400,"height_mm":80}', '{"width_mm":50,"height_mm":80,"device":"private"}']) {
      localStorage.setItem("openlabel.paper.v1", raw); expect(readPaper()).toEqual({});
    }
    for (const width_mm of [NaN, Infinity, -Infinity, 51]) expect(writePaper({ width_mm, height_mm: 80 })).toBeNull();
    expect(writePaper({ width_mm: 50, height_mm: 80 })).toBe(true); expect(readPaper().paper).toEqual({ width_mm: 50, height_mm: 80 });
  });
  it.each(["read", "write"])("storage %s failure remains usable and reports recovery", async failure => {
    const get = vi.spyOn(localStorage, "getItem"), set = vi.spyOn(localStorage, "setItem");
    if (failure === "read") get.mockImplementation(() => { throw new Error("unavailable"); });
    set.mockImplementation(() => { throw new Error("quota"); });
    mockMediaPreview(); render(<App />); await openAndSelect();
    expect(screen.getByText(/용지를 다음 실행에 기억하지 못했습니다/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("용지 높이 (mm)"), { target: { value: "80" } });
    mocks.open.mockResolvedValue("/other.png"); fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/50 × 80mm/);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByText("보정 패턴")); await screen.findByAltText(/50 × 80mm/);
    expect(screen.getByText("용지를 다음 실행에 기억하지 못했습니다. 현재 창에서는 유지됩니다. 용지 값을 수정하거나 이미지 화면에서 새로고침하세요.")).toBeTruthy();
    fireEvent.click(screen.getByText("라벨 보기")); await screen.findByAltText(/50 × 80mm/);
    expect(screen.getByText(/용지를 다음 실행에 기억하지 못했습니다/)).toBeTruthy();
    get.mockRestore(); set.mockRestore(); fireEvent.click(screen.getByText("미리보기 새로고침")); await screen.findByAltText(/50 × 80mm/);
    expect(screen.queryByText(/용지를 다음 실행에 기억하지 못했습니다/)).toBeNull(); expect(storedPaper()).toEqual({ width_mm: 50, height_mm: 80 });
  });
});
describe("review flow", () => {
  it("groups controls in DOM order and keeps copies beside Print outside scrolling settings", async () => {
    render(<App />); await openAndSelect();
    const width = screen.getByLabelText("용지 폭 (mm)"), height = screen.getByLabelText("용지 높이 (mm)"), scale = screen.getByLabelText("이미지 크기 (%)"), fit = screen.getByText("이미지 맞춤");
    expect(width.closest(".settings-grid")).toBe(height.closest(".settings-grid"));
    expect(scale.closest(".settings-grid")).toBe(fit.parentElement);
    expect(scale.closest(".settings-grid")).not.toBe(width.closest(".settings-grid"));
    expect(scale.compareDocumentPosition(fit) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const copies = screen.getByLabelText("인쇄 매수"), print = screen.getByText("1장 인쇄");
    expect(copies.closest(".print-row")).toBe(print.parentElement);
    expect(copies.closest(".inspector-scroll")).toBeNull();
    expect(copies.compareDocumentPosition(print) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole("status").closest(".print-action")).toBeTruthy();
    expect([...document.querySelectorAll(".advanced .control-group h3")].map(node => node.textContent)).toEqual(["이미지 출력", "위치·보정", "농도·속도", "설정 파일", "패턴·새로고침"]);
    expect(choice("회전").closest("section")?.querySelector("h2")?.textContent).toBe("이미지");
    for (const [label, heading] of [["X 보정 (mm)", "위치·보정"], ["농도", "농도·속도"]]) {
      const control = screen.getByLabelText(label);
      expect(control.closest(".control-group")?.querySelector("h3")?.textContent).toBe(heading);
    }
    expect(document.querySelector(".accordion .accordion")).toBeNull();
  });
  it("retains long synthetic printer identity and connection error inside the printer disclosure", async () => {
    const name = "M110-" + "synthetic-name".repeat(30), id = "synthetic-id".repeat(30), detail = "synthetic-connection-error".repeat(30);
    const runtime = runtimeFixture(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "scan_devices" ? Promise.resolve([{ id, name, rssi: -50, model_allowed: true }]) : base(command, args));
    render(<App />); fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/최종 흑백/);
    reveal(screen.getByText("Bluetooth 검색")); fireEvent.click(screen.getByText("Bluetooth 검색")); await choices("장치 선택");
    expect(screen.getByRole("option", { name: `${name} · ${id} · -50 dBm (후보)` })).toBeTruthy();
    await choose("장치 선택", id);
    expect(document.querySelector(".printer-name")?.textContent).toContain(name);
    expect(document.querySelector(".printer .accordion__trigger > span > span")?.textContent).toBe("프린터 · 연결 안 됨");
    expect(choice("장치 선택").textContent).toContain(id);
    runtime.connection = { ...runtime.connection, error: { code: "disconnected", detail } };
    const error = await screen.findByText(detail);
    expect(error.closest(".printer")).toBeTruthy(); expect(error.closest(".print-action")).toBeNull();
    expect((screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인") as HTMLInputElement).checked).toBe(false);
  });
  it("fit and integer-dot scrolling modes preserve raster and do not request another preview", async () => {
    render(<App />); await openAndSelect();
    const img = screen.getByAltText(/최종 흑백/) as HTMLImageElement, viewport = img.closest(".preview-scroll")!;
    const source = img.src, count = mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").length;
    expect(viewport.classList.contains("fit")).toBe(true);
    for (const zoom of [1, 2, 4]) {
      await choose("화면 확대", String(zoom));
      expect(viewport.classList.contains("fit")).toBe(false);
      expect(img.style.width).toBe(`${384 * zoom}px`); expect(img.style.height).toBe(`${240 * zoom}px`);
    }
    fireEvent.click(screen.getByLabelText("head 전체 보기"));
    expect((document.querySelector(".paper-scene") as HTMLElement).style.width).toBe("1536px");
    await choose("화면 확대", "fit");
    expect(viewport.classList.contains("fit")).toBe(true); expect(img.src).toBe(source);
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label")).toHaveLength(count);
  });
  it("declares shrink/wrap boundaries, input-edge alignment and distinct fit overflow without claiming pixels", async () => {
    const { readFileSync } = await vi.importActual<{ readFileSync: (path: string, encoding: string) => string }>("node:fs");
    const styles = readFileSync("src/styles.css", "utf8");
    expect(styles).toMatch(/\.settings-grid \{[^}]*align-items: end/);
    expect(styles).toMatch(/\.print-row \{[^}]*grid-template-columns: 80px minmax\(0, 1fr\)[^}]*align-items: end/);
    expect(styles).toMatch(/\.inspector-scroll \{[^}]*overflow-x: hidden; overflow-y: auto;[^}]*padding: var\(--space-2\); scrollbar-gutter: stable/);
    expect(styles).toMatch(/\.inspector-scroll \{[^}]*scroll-padding-block: var\(--space-2\)/);
    expect(styles).toContain(".inspector-scroll :is(button, input) { scroll-margin-block: var(--space-2); }");
    expect(styles).toContain("@media (max-width: 850px) { html { scroll-padding-block-end: 50dvh; }");
    expect(styles).toMatch(/\.inspector :is\([^}]+max-width: 100%; overflow-wrap: anywhere/);
    expect(styles).toMatch(/\.settings-grid > \*[^}]+min-width: 0; max-width: 100%/);
    expect(styles).toMatch(/\.preview-scroll \{[^}]*overflow: auto/);
    expect(styles).toContain(".preview-scroll.fit { overflow: hidden; }");
    expect(styles).toContain(".input { height: var(--target-size); }");
  });
  it.each([false, true])("late link response cannot overwrite fresher runtime, rejected=%s", async rejected => {
    const runtime = runtimeFixture(), pending = deferred<unknown>(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "connect_device" ? pending.promise : base(command, args));
    render(<App />); await openAndSelect(); fireEvent.click(screen.getByText("연결"));
    runtime.revision++;
    runtime.connection = { ...runtime.connection, error: { code: "disconnected", detail: "new native disconnect" } };
    await screen.findByText("new native disconnect");
    await act(async () => { if (rejected) pending.reject({ detail: "stale link error" }); else pending.resolve({ state: "connected", device: "exact-id", evidence: null, error: null }); });
    expect(document.querySelector(".printer .accordion__trigger")?.textContent).toContain("연결 안 됨");
    expect(screen.queryByText("stale link error")).toBeNull();
    expect(screen.getByText("new native disconnect")).toBeTruthy();
    expect((screen.getByText("연결") as HTMLButtonElement).disabled).toBe(false);
  });
  it("idle disconnect cleanup with busy false locks mutations until native recovery", async () => {
    const runtime = runtimeFixture(), base = mocks.invoke.getMockImplementation()!; let failedPoll = false;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? failedPoll ? Promise.reject({ detail: "cleanup poll lost" }) : Promise.resolve(runtime) : base(command, args));
    render(<App />); await openAndSelect();
    runtime.connection = { state: "disconnecting", device: "exact-id", evidence: { device_id: "exact-id", model: "M110", service: "FF00", characteristic: "FF02", write_type: "without-response", mtu: 131 }, error: null };
    expect(runtime.busy).toBe(false); expect(runtime.job).toBeNull();
    await waitFor(() => expect(document.querySelector(".printer .accordion__trigger")?.textContent).toContain("해제 중"));
    expectMutationsLocked(); expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
    failedPoll = true;
    await screen.findByText("cleanup poll lost"); expectMutationsLocked();
    failedPoll = false;
    runtime.connection = runtimeFixture().connection;
    await waitFor(() => expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(false));
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
  });
  it("fresh link rejection remains actionable and recovers on native truth", async () => {
    const runtime = runtimeFixture(), pending = deferred<unknown>(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "connect_device" ? pending.promise : base(command, args));
    render(<App />); await openAndSelect(); fireEvent.click(screen.getByText("연결"));
    await act(async () => pending.reject({ detail: "fresh connection failure" }));
    expect(screen.getByText("fresh connection failure")).toBeTruthy();
    runtime.connection = { ...runtime.connection, error: { code: "disconnected", detail: "native connection failure" } };
    await screen.findByText("native connection failure");
    expect((screen.getByText("연결") as HTMLButtonElement).disabled).toBe(false);
  });
  it("preparing acknowledgement cannot regress observed sending bytes", async () => {
    const runtime = runtimeFixture(), pending = deferred<Job>(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "start_print" ? pending.promise : base(command, args));
    render(<App />); await openAndSelect(); fireEvent.click(screen.getByText("1장 인쇄"));
    runtime.job = { ...jobFixture(1, "desktop", "sending", false), sent_bytes: 7 }; runtime.desktop_job = runtime.job; runtime.busy = true;
    await screen.findByText("인쇄 전송 중");
    await act(async () => pending.resolve(jobFixture(1, "desktop", "preparing", false)));
    expect(screen.getByRole("status").textContent).toContain("인쇄 전송 중");
    expect(Number(screen.getByRole("progressbar").getAttribute("aria-valuenow"))).toBe(7);
  });
  it("runtime failures clear verified connection without replacing a local source error", async () => {
    const runtime = runtimeFixture(); let fail = false;
    runtime.connection = { state: "connected", device: "exact-id", evidence: { device_id: "exact-id", model: "M110", service: "FF00", characteristic: "FF02", write_type: "without-response", mtu: 131 }, error: null };
    mocks.invoke.mockImplementation(command => command === "get_runtime" ? fail ? Promise.reject({ detail: "runtime transport lost" }) : Promise.resolve(runtime) : Promise.reject({ code: "invalid_label", detail: "local source error" }));
    render(<App />); fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("local source error"));
    expect(document.querySelector(".printer .accordion__trigger")?.textContent).toContain("연결됨");
    fail = true;
    await waitFor(() => expect(document.querySelector(".printer")?.textContent).toContain("runtime transport lost"));
    expect(document.querySelector(".printer .accordion__trigger")?.textContent).not.toContain("연결됨");
    expect(document.querySelector(".printer .accordion__trigger")?.textContent).toContain("상태 확인 필요");
    expect(screen.getByRole("status").textContent).toContain("local source error");
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
  });
  it("a delayed own acknowledgement cannot regress an already observed terminal", async () => {
    const runtime = runtimeFixture(), pending = deferred<Job>(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "start_print" ? pending.promise : base(command, args));
    render(<App />); await openAndSelect(); fireEvent.click(screen.getByText("1장 인쇄"));
    runtime.job = jobFixture(1, "desktop", "completed", true); runtime.desktop_job = runtime.job;
    await screen.findByText("인쇄 완료");
    await act(async () => pending.resolve(jobFixture(1, "desktop", "preparing", false)));
    expect(screen.getByRole("status").textContent).toContain("인쇄 완료");
    expect(screen.queryByText("취소")).toBeNull();
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(false);
  });
  it("an older start acknowledgement cannot release a newer owned pending start", async () => {
    const runtime = runtimeFixture(), first = deferred<Job>(), second = deferred<Job>(), base = mocks.invoke.getMockImplementation()!;
    let starts = 0;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "start_print" ? ++starts === 1 ? first.promise : second.promise : base(command, args));
    render(<App />); await openAndSelect(); fireEvent.click(screen.getByText("1장 인쇄"));
    runtime.job = jobFixture(1, "desktop", "completed", true); runtime.desktop_job = runtime.job;
    await screen.findByText("인쇄 완료"); fireEvent.click(screen.getByText("1장 인쇄"));
    expect(starts).toBe(2);
    await act(async () => first.resolve(jobFixture(1, "desktop", "preparing", false)));
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(true);
    runtime.job = jobFixture(2, "desktop", "completed", true); runtime.desktop_job = runtime.job;
    await waitFor(() => expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(false));
    await act(async () => second.resolve(jobFixture(2, "desktop", "preparing", false)));
    expect(screen.getByRole("status").textContent).toContain("인쇄 완료");
    expect(screen.queryByText("취소")).toBeNull();
  });
  it("exposes retained native evidence without auto-selection and preserves confirmation during scan", async () => {
    const runtime = runtimeFixture();
    runtime.connection = { state: "connected", device: "retained-id", evidence: { device_id: "retained-id", model: "M110", service: "FF00", characteristic: "FF02", write_type: "without-response", mtu: 131 }, error: null };
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "scan_devices" ? Promise.resolve([]) : base(command, args));
    render(<App />); fireEvent.click(screen.getByText("이미지 열기")); await screen.findByAltText(/최종 흑백/);
    await choices("장치 선택"); expect(screen.getByRole("option", { name: /M110 · retained-id/ })).toBeTruthy();
    expect(choice("장치 선택").textContent).toContain("프린터 선택");
    expect((screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인") as HTMLInputElement).checked).toBe(false);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
    await choose("장치 선택", "retained-id");
    fireEvent.click(screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인"));
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByText("Bluetooth 검색"));
    await waitFor(() => expect((screen.getByText("Bluetooth 검색") as HTMLButtonElement).disabled).toBe(false));
    expect(choice("장치 선택").textContent).toContain("retained-id");
    expect((screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인") as HTMLInputElement).checked).toBe(true);
    fireEvent.change(screen.getByLabelText("이미지 크기 (%)"), { target: { value: "80" } });
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
    expect(mocks.invoke.mock.calls.some(c => c[0] === "get_job")).toBe(false);
  });
  it("external hash failure never invalidates the human image and is dismissed by explicit action", async () => {
    const runtime = runtimeFixture(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : base(command, args));
    render(<App />); await openAndSelect();
    runtime.job = jobFixture(1, "cli", "preparing", false); runtime.busy = true;
    await screen.findByText(/CLI · 입력/); expectMutationsLocked();
    runtime.job = { ...jobFixture(1, "cli", "failed", true), error: { code: "hash_mismatch", detail: "external source changed" } }; runtime.busy = false;
    await screen.findByText("external source changed"); expect(screen.getByAltText(/최종 흑백/)).toBeTruthy();
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.change(screen.getByLabelText("인쇄 매수"), { target: { value: "2" } });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 220)); });
    expect(screen.queryByText("external source changed")).toBeNull();
    expect((screen.getByText("2장 인쇄") as HTMLButtonElement).disabled).toBe(false);
  });
  it.each([false, true])("owns final snapshot across newer CLI job and delayed acknowledgement, mismatch=%s", async mismatch => {
    const runtime = runtimeFixture(), acknowledgement = deferred<Job>(), base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === "start_print" ? acknowledgement.promise : base(command, args));
    render(<App />); await openAndSelect(); fireEvent.click(screen.getByText("1장 인쇄"));
    runtime.desktop_job = { ...jobFixture(1, "desktop", mismatch ? "failed" : "completed", true), error: mismatch ? { code: "hash_mismatch", detail: "own source changed" } : null };
    runtime.job = jobFixture(2, "cli", "sending", false); runtime.busy = true;
    await screen.findByText(/CLI · 인쇄 전송 중/);
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(true);
    expect(!!screen.queryByAltText(/최종 흑백/)).toBe(!mismatch);
    await act(async () => acknowledgement.resolve(jobFixture(1, "desktop", "preparing", false)));
    expect(screen.getByRole("status").textContent).toContain("CLI · 인쇄 전송 중");
    runtime.job = jobFixture(2, "cli", "completed", true); runtime.busy = false;
    await waitFor(() => expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(false));
    if (mismatch) { fireEvent.click(screen.getByText("미리보기 새로고침")); await screen.findByAltText(/최종 흑백/); }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 220)); });
    expect(screen.getByAltText(/최종 흑백/)).toBeTruthy();
  });
  it.each(["scan-success", "scan-error", "save-success", "save-error"])("releases local %s ownership after an external job supersedes its epoch", async mode => {
    const runtime = runtimeFixture(), pending = deferred<unknown>(), base = mocks.invoke.getMockImplementation()!;
    mocks.save.mockResolvedValue("/synthetic.openlabel.json");
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : base(command, args));
    render(<App />); await openAndSelect();
    mocks.invoke.mockImplementation((command, args) => command === "get_runtime" ? Promise.resolve(runtime) : command === (mode.startsWith("scan") ? "scan_devices" : "save_settings") ? pending.promise : base(command, args));
    fireEvent.click(screen.getByText(mode.startsWith("scan") ? "Bluetooth 검색" : "설정 저장"));
    await waitFor(() => expect(mocks.invoke.mock.calls.some(c => c[0] === (mode.startsWith("scan") ? "scan_devices" : "save_settings"))).toBe(true));
    runtime.job = jobFixture(4, "cli", "sending", false); runtime.busy = true;
    await screen.findByText(/CLI · 인쇄 전송 중/);
    runtime.job = jobFixture(4, "cli", "completed", true); runtime.busy = false;
    await screen.findByText("인쇄 완료");
    await act(async () => { if (mode.endsWith("error")) pending.reject({ detail: "stale operation error" }); else pending.resolve(mode.startsWith("scan") ? [] : { ...fixture(), sha256: "stale saved raster" }); });
    await waitFor(() => expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(false));
    expect(screen.queryByText(/stale/)).toBeNull(); expect(screen.getByAltText(/최종 흑백/)).toBeTruthy();
    expect(choice("장치 선택").textContent).toContain("exact-id");
  });
  it("keeps a concise visible error and the complete native detail in disclosure", async () => {
    const detail = "손상된 이미지: " + "decoder detail ".repeat(30);
    mocks.invoke.mockRejectedValue({code:"invalid_label",detail});
    render(<App />); fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("작업을 완료할 수 없습니다. 상세 정보를 확인하세요."));
    const disclosure = screen.getByRole("button", { name: "상세 정보" });
    expect(disclosure.getAttribute("aria-expanded")).toBe("false"); expect(disclosure.closest(".accordion")?.textContent).toContain(detail);
    expect(screen.queryByText("미리보기 준비 중")).toBeNull();
  });
  it("keeps technical content closed and image dialog/drop/save naming consistent", async () => {
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByAltText(/최종 흑백/);
    expect(mocks.open.mock.calls[0][0].filters[0].extensions).toEqual(["svg", "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"]);
    expect((screen.getByLabelText("이미지 크기 (%)") as HTMLInputElement).value).toBe("100");
    expect(mocks.invoke.mock.calls.find(c => c[0] === "preview_label")![1].request.overrides).toEqual({});
    for (const text of ["고급 설정", "상세 정보"]) expect(screen.getByRole("button", { name: text }).getAttribute("aria-expanded")).toBe("false");
    for (const text of ["raster", "/label.svg"]) expect(screen.getByText(text).closest(".accordion")?.querySelector(".accordion__trigger")?.getAttribute("aria-expanded")).toBe("false");
    expect(screen.getAllByRole("status")).toHaveLength(1);
    const callback = mocks.drop.mock.calls[0][0];
    for (const [source, target] of [["logo.svg","logo.openlabel.json"], ["upper.SVG","upper.openlabel.json"], ["logo.png","logo.png.openlabel-image.json"], ["photo.JPEG","photo.JPEG.openlabel-image.json"], ["page.tif","page.tif.openlabel-image.json"], ["photo.png.svg","photo.png.openlabel.json"], ["report","report.openlabel-image.json"], ["report.svg","report.openlabel.json"], [".svg",".svg.openlabel-image.json"], [".SVG",".SVG.openlabel-image.json"]]) {
      await act(async () => callback({ payload: { type: "drop", paths: [`/synthetic/${source}`] } }));
      await screen.findByAltText(/최종 흑백/);
      fireEvent.click(screen.getByText("설정 저장"));
      await waitFor(() => expect(mocks.save).toHaveBeenLastCalledWith(expect.objectContaining({defaultPath: `/synthetic/${target}`})));
      expect(document.querySelector(".filename")?.textContent).toBe(source);
    }
  });
  it.each([false, true])("preserves sidecar scale across pattern with initial preview resolved=%s", async (resolved) => {
    const initial = deferred<Preview>();
    const artwork = fixture(); artwork.settings.layout.scale_percent = 75.5;
    let externalCalls = 0;
    mocks.invoke.mockImplementation((command, args) => {
      if (command !== "preview_label") return Promise.resolve(null);
      if (args.request.test_pattern) return Promise.resolve(fixture());
      return ++externalCalls === 1 ? initial.promise : Promise.resolve(artwork);
    });
    render(<App />); fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(externalCalls).toBe(1));
    expect(screen.getByText("미리보기 준비 중")).toBeTruthy();
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect(screen.getByRole("status").textContent).toContain("미리보기 계산 중…");
    if (resolved) await act(async () => initial.resolve(artwork));
    fireEvent.click(screen.getByText("보정 패턴"));
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByLabelText("이미지 크기 (%)") as HTMLInputElement).value).toBe("100");
    fireEvent.click(screen.getByText("라벨 보기"));
    await waitFor(() => expect(externalCalls).toBe(2));
    const request = mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").at(-1)![1].request;
    if (resolved) expect(request.overrides.scale_percent).toBe(75.5);
    else expect(request.overrides).not.toHaveProperty("scale_percent");
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByLabelText("이미지 크기 (%)") as HTMLInputElement).value).toBe("75.5");
    if (!resolved) await act(async () => initial.resolve(fixture()));
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect((screen.getByLabelText("이미지 크기 (%)") as HTMLInputElement).value).toBe("75.5");
  });
  it.each(["50", "150", "201", ""])("preserves artwork scale %s across fixed-scale patterns", async (value) => {
    render(<App />); await openAndSelect();
    mocks.invoke.mockImplementation((command, args) => {
      if (command !== "preview_label") return Promise.resolve(null);
      const p = fixture(); p.settings.layout.scale_percent = args.request.overrides.scale_percent ?? 100;
      p.sha256 = args.request.test_pattern ? "fixed-40-dot-ruler" : "scaled-artwork";
      return Promise.resolve(p);
    });
    fireEvent.change(screen.getByLabelText("이미지 크기 (%)"), {target:{value}});
    fireEvent.click(screen.getByText("보정 패턴"));
    await screen.findByText("fixed-40-dot-ruler");
    const scale = screen.getByLabelText("이미지 크기 (%)") as HTMLInputElement;
    expect(scale.value).toBe("100"); expect(scale.disabled).toBe(true);
    expect((screen.getByText("이미지 맞춤") as HTMLButtonElement).disabled).toBe(true);
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").at(-1)![1].request.overrides.scale_percent).toBe(100);
    fireEvent.click(screen.getByText("라벨 보기"));
    expect(scale.value).toBe(value);
    if (value === "201" || value === "") {
      expect(screen.getByRole("status").textContent).toContain("숫자 입력값");
      expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
      expect(screen.queryByAltText(/최종 흑백/)).toBeNull();
    } else await screen.findByText("scaled-artwork");
  });
  it("paper/scale regenerate PNG, fractional size works and Fit retains paper/calibration", async () => {
    render(<App />); await openAndSelect();
    const fitting = deferred<Preview>();
    let fitResult: Preview | undefined;
    mocks.invoke.mockImplementation((command, args) => {
      if (command !== "preview_label") return Promise.resolve(null);
      const p = fixture();
      p.settings.paper.width_mm = args.request.overrides.width_mm ?? 40;
      p.settings.layout.scale_percent = args.request.overrides.scale_percent ?? 100;
      p.png_base64 = btoa(JSON.stringify(args.request.overrides));
      if (args.request.overrides.scale_percent === 100) { fitResult = p; return fitting.promise; }
      return Promise.resolve(p);
    });
    const original = (screen.getByAltText(/최종 흑백/) as HTMLImageElement).src;
    fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), {target:{value:"45"}});
    fireEvent.change(screen.getByLabelText("X 보정 (mm)"), {target:{value:"0.5"}});
    fireEvent.change(screen.getByLabelText("이미지 크기 (%)"), {target:{value:"150.5"}});
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByAltText(/최종 흑백/) as HTMLImageElement).src).not.toBe(original);
    expect(screen.getByRole("status").textContent).toBe("일부 내용이 잘릴 수 있습니다");
    fireEvent.click(screen.getByText("이미지 맞춤"));
    await waitFor(() => expect(fitResult).toBeDefined());
    fireEvent.click(screen.getByText("이미지 맞춤"));
    await act(async () => fitting.resolve(fitResult!));
    await screen.findByAltText(/최종 흑백/);
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").at(-1)![1].request.overrides).toEqual({width_mm:45,offset_x_mm:0.5,scale_percent:100});
    expect(screen.getByRole("status").textContent).toBe("");
    const fitted = screen.getByAltText(/최종 흑백/);
    const calls = mocks.invoke.mock.calls.length;
    fireEvent.click(screen.getByText("이미지 맞춤"));
    expect(screen.getByAltText(/최종 흑백/)).toBe(fitted);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
    expect(mocks.invoke.mock.calls).toHaveLength(calls);
    fireEvent.change(screen.getByLabelText("이미지 크기 (%)"), {target:{value:"201"}});
    expect((screen.getByLabelText("이미지 크기 (%)") as HTMLInputElement).value).toBe("201");
    expect(screen.getByRole("status").textContent).toContain("숫자 입력값");
  });
  it("exposes invalid Advanced controls and user switching Floyd clears invalid threshold", async () => {
    render(<App />); await openAndSelect();
    const details = screen.getByRole("button", { name: "고급 설정" });
    expect(details.getAttribute("aria-expanded")).toBe("false");
    fireEvent.change(screen.getByLabelText("임계값", { selector: "input" }), {target:{value:""}});
    expect(details.getAttribute("aria-expanded")).toBe("true");
    expect((screen.getByLabelText("임계값", { selector: "input" }) as HTMLInputElement).value).toBe("");
    await choose("흑백 방식", "floyd-steinberg");
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByLabelText("임계값", { selector: "input" }) as HTMLInputElement).value).toBe("128");
    expect((screen.getByLabelText("임계값", { selector: "input" }) as HTMLInputElement).disabled).toBe(true);
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").at(-1)![1].request.overrides.threshold).toBe(128);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
    expect(details.getAttribute("aria-expanded")).toBe("true");
    // Same-key Select re-selection emits no change; preserve the hook's synthetic same-mode invariant directly.
    cleanup(); const { result } = renderHook(() => usePrintWorkflow());
    act(() => result.current.change("raster_mode", "floyd-steinberg"));
    act(() => result.current.change("threshold", NaN));
    expect(result.current.overrides.threshold).toBeNaN();
    act(() => result.current.change("raster_mode", "floyd-steinberg"));
    expect(result.current.overrides.threshold).toBe(128);
  });
  it("Advanced keeps manual state and focus through blank, first valid prefix and completed edit", async () => {
    render(<App />); await openAndSelect();
    const summary = screen.getByText("고급 설정"), details = screen.getByRole("button", { name: "고급 설정" });
    expect(details.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(summary); expect(details.getAttribute("aria-expanded")).toBe("true");
    fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), {target:{value:"45"}});
    expect(details.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(summary); expect(details.getAttribute("aria-expanded")).toBe("false");
    const density = screen.getByLabelText("농도");
    fireEvent.change(density, {target:{value:""}});
    expect(details.getAttribute("aria-expanded")).toBe("true");
    density.focus();
    for (const value of ["1", "12"]) {
      fireEvent.change(density, {target:{value}});
      expect(details.getAttribute("aria-expanded")).toBe("true"); expect(document.activeElement).toBe(density);
    }
    await screen.findByAltText(/최종 흑백/);
    expect(details.getAttribute("aria-expanded")).toBe("true"); expect(document.activeElement).toBe(density);
    fireEvent.click(summary); expect(details.getAttribute("aria-expanded")).toBe("false");
  });
  it("exposes main-panel refresh while terminal source invalidation stays locked through cleanup", async () => {
    render(<App />); await openAndSelect();
    let finished = false;
    mocks.invoke.mockImplementation(command => command === "preview_label" ? Promise.resolve(fixture()) : Promise.resolve({id:99,state:"failed",finished,error:{code:"hash_mismatch",detail:"이미지가 변경되었습니다."}}));
    fireEvent.click(screen.getByText("1장 인쇄"));
    await waitFor(() => expect(screen.queryByAltText(/최종 흑백/)).toBeNull());
    const refresh = screen.getByText("미리보기 새로고침") as HTMLButtonElement;
    expect(refresh.closest(".accordion__panel")).toBeNull(); expect(refresh.disabled).toBe(true);
    expect(screen.queryByText("미리보기 준비 중")).toBeNull();
    finished = true;
    await waitFor(() => expect(refresh.disabled).toBe(false));
    fireEvent.click(refresh); await screen.findByAltText(/최종 흑백/);
  });
  it.each(["completed", "failed", "cancelled"])("publishes immutable %s before cleanup finishes", async (outcome) => {
    render(<App />); await openAndSelect();
    const cancellation = deferred<void>();
    let mode = "poll-error", finished = false, id = 6;
    const notice = { completed: "인쇄 완료", failed: "인쇄 실패", cancelled: "인쇄 취소" }[outcome]!;
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "start_print") return Promise.resolve({ id: ++id, state: "sending", finished: false });
      if (command === "cancel_job") return cancellation.promise;
      if (command === "get_runtime") {
        if (mode === "poll-error") return Promise.reject({ detail: "pre-terminal-poll-error" });
        if (mode === "late-error") return Promise.reject({ detail: "late-terminal-poll-error" });
        return Promise.resolve({ id, state: outcome, finished });
      }
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("1장 인쇄"));
    await screen.findByText("취소");
    fireEvent.click(screen.getByText("취소"));
    await waitFor(() => expect(document.querySelector(".printer")?.textContent).toContain("pre-terminal-poll-error"));
    expect(screen.getByRole("status").textContent).not.toContain("pre-terminal-poll-error");
    mode = "terminal";
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain(notice));
    expect(screen.getByRole("status").textContent).not.toContain("pre-terminal-poll-error");
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByText("취소") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByText("취소"));
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "cancel_job")).toHaveLength(1);
    await act(async () => cancellation.reject({ detail: "late-terminal-cancel-error" }));
    expect(screen.getByRole("status").textContent).toContain(notice);
    mode = "late-error";
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 200)); });
    expect(screen.getByRole("status").textContent).toContain(notice);
    expect(screen.getByRole("status").textContent).not.toContain("late-terminal");
    mode = "terminal";
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 200)); });
    expect(screen.getByRole("status").textContent).toContain(notice);
    finished = true;
    await waitFor(() => expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false));
    expect(screen.getByRole("status").textContent).toContain(notice);
    if (outcome === "completed") {
      fireEvent.click(screen.getByText("1장 인쇄"));
      const starts = mocks.invoke.mock.calls.filter(c => c[0] === "start_print");
      expect(starts).toHaveLength(2);
      expect(starts[1][1]).toEqual(starts[0][1]);
      await waitFor(() => expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false));
    }
    fireEvent.click(screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인"));
    expect(screen.getByRole("status").textContent).not.toContain(notice);
  });
  it.each(["start-rejection", "job-mismatch"])("clears stale ready after %s without automatic render", async (failure) => {
    render(<App />); await openAndSelect();
    const before = mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").length;
    const error = { code: "hash_mismatch", detail: "stale input" };
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "start_print") return failure === "start-rejection" ? Promise.reject(error) : Promise.resolve({ id: 11, state: "preparing", finished: false });
      if (command === "get_runtime") return Promise.resolve({ id: 11, state: "failed", finished: true, error });
      if (command === "preview_label") return Promise.resolve(fixture());
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("1장 인쇄"));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("stale input"));
    expect(screen.queryByAltText(/최종 흑백/)).toBeNull();
    expect(screen.getByText("미리보기 새로고침").closest(".accordion__panel")).toBeNull();
    expect(screen.queryByText("미리보기 준비 중")).toBeNull();
    fireEvent.click(screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인"));
    expect(screen.getByRole("status").textContent).not.toContain("미리보기 준비됨");
    expect(screen.getByRole("status").textContent).toContain("미리보기를 새로고침");
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 180)); });
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label")).toHaveLength(before);
    fireEvent.click(screen.getByLabelText("선택한 프린터 본체의 M110 모델 확인"));
    fireEvent.click(screen.getByText("미리보기 새로고침"));
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(false);
  });
  it.each([true, false])("keeps discovery owned when preview settles first=%s", async (previewFirst) => {
    const preview = deferred<Preview>(), scan = deferred<unknown>();
    mocks.invoke.mockImplementation((command: string) => command === "preview_label" ? preview.promise : scan.promise);
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("preview_label", expect.anything()));
    fireEvent.click(screen.getByText("Bluetooth 검색"));
    if (previewFirst) {
      await act(async () => preview.resolve(fixture()));
      expect(screen.getByRole("status").textContent).toContain("Bluetooth 후보 검색 중");
      expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(true);
    }
    await act(async () => scan.reject({ detail: "독립 검색 오류" }));
    if (!previewFirst) await act(async () => preview.resolve(fixture()));
    expect(screen.getByRole("status").textContent).toContain("독립 검색 오류");
    expect(screen.getByAltText(/최종 흑백/)).toBeTruthy();
  });
  it.each([true, false])("ignores superseded dialog success=%s and previous preview rejection", async (success) => {
    render(<App />);
    await openAndSelect();
    const dialog = deferred<unknown>(), oldPreview = deferred<Preview>();
    mocks.open.mockReturnValueOnce(dialog.promise).mockResolvedValue("/new.svg");
    fireEvent.click(screen.getByText("설정 불러오기"));
    mocks.invoke.mockImplementation((command: string, args: { request: { path: string } }) => command === "preview_label" && args.request.path === "/label.svg" ? oldPreview.promise : Promise.resolve({ ...fixture(), sha256: "new-raster" }));
    fireEvent.click(screen.getByText("미리보기 새로고침"));
    await waitFor(() => expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label")).toHaveLength(2));
    fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByText("new-raster");
    await act(async () => { if (success) dialog.resolve("/stale.json"); else dialog.reject({ detail: "stale dialog error" }); oldPreview.reject({ detail: "stale preview error" }); });
    expect(screen.getByText("new-raster")).toBeTruthy();
    expect(screen.queryByText(/stale/)).toBeNull();
  });
  it("ignores an unmounted drop callback and disposes a late subscription", async () => {
    const subscription = deferred<() => void>(), unsubscribe = vi.fn();
    mocks.drop.mockReturnValue(subscription.promise);
    const view = render(<App />);
    const callback = mocks.drop.mock.calls[0][0];
    view.unmount();
    await act(async () => { callback({ payload: { type: "drop", paths: ["/late.svg"] } }); subscription.resolve(unsubscribe); });
    expect(unsubscribe).toHaveBeenCalledTimes(1);
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
  it("repeats a completed print with one activation and an unchanged full request", async () => {
    render(<App />); await openAndSelect();
    let id = 0;
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "start_print") return Promise.resolve({ id: ++id, state: "preparing", finished: false });
      if (command === "get_runtime") return Promise.resolve({ id, state: "completed", finished: true });
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("1장 인쇄"));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("인쇄 완료"));
    fireEvent.click(screen.getByText("1장 인쇄"));
    fireEvent.click(screen.getByText("1장 인쇄"));
    const starts = mocks.invoke.mock.calls.filter(c => c[0] === "start_print");
    expect(starts).toHaveLength(2);
    expect(starts[1][1]).toEqual(starts[0][1]);
    expect(starts[1][1].request.expect_input_sha256).toBe("input");
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("인쇄 완료"));
  });
  it("owns sending/poll errors, ignores old IDs and late cancel rejection after cleanup", async () => {
    render(<App />); await openAndSelect();
    const cancellation = deferred<void>();
    let state = "old";
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "start_print") return Promise.resolve({ id: 4, state: "preparing", finished: false });
      if (command === "cancel_job") return cancellation.promise;
      if (command === "get_runtime") {
        if (state === "error") return Promise.reject({ detail: "poll-owned-error" });
        return Promise.resolve({ id: state === "old" ? 3 : 4, state: state === "done" ? "completed" : "sending", finished: state === "done" });
      }
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("1장 인쇄"));
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 200)); });
    expect(screen.getByRole("status").textContent).toContain("입력 재검증·연결 중");
    state = "sending";
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("인쇄 전송 중"));
    state = "error";
    await waitFor(() => expect(document.querySelector(".printer")?.textContent).toContain("poll-owned-error"));
    expect(screen.getByRole("status").textContent).toContain("인쇄 전송 중");
    state = "sending";
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 200)); });
    expect(document.querySelector(".printer")?.textContent).not.toContain("poll-owned-error");
    expect(screen.getByRole("status").textContent).toContain("인쇄 전송 중");
    fireEvent.click(screen.getByText("취소"));
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(true);
    state = "done";
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("인쇄 완료"));
    await act(async () => cancellation.reject({ detail: "late-cancel-error" }));
    expect(screen.getByRole("status").textContent).toContain("인쇄 완료");
    expect(screen.queryByText(/late-cancel/)).toBeNull();
  });
  it("keeps unknown and unnamed candidates explicit and clears model attestation on selection change", async () => {
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "preview_label") return Promise.resolve(fixture());
      if (command === "scan_devices")
        return Promise.resolve([
          {
            id: "synthetic-one",
            name: "Renamed device",
            rssi: -60,
            supported: false,
            model_allowed: true,
          },
          {
            id: "synthetic-two",
            name: null,
            rssi: null,
            supported: false,
            model_allowed: true,
          },
        ]);
      return Promise.resolve(null);
    });
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByAltText(/최종 흑백/);
    fireEvent.click(screen.getByText("Bluetooth 검색"));
    await choices("장치 선택");
    expect(screen.getByRole("option", { name: /Renamed device · synthetic-one/ })).toBeTruthy();
    expect(screen.getByRole("option", { name: /이름 없음 · synthetic-two/ })).toBeTruthy();
    const selection = choice("장치 선택");
    const attestation = screen.getByLabelText(
      "선택한 프린터 본체의 M110 모델 확인",
    ) as HTMLInputElement;
    const print = screen.getByText("1장 인쇄") as HTMLButtonElement;
    expect(selection.textContent).toContain("프린터 선택");
    expect(attestation.checked).toBe(false);
    expect(print.disabled).toBe(true);
    await choose("장치 선택", "synthetic-one");
    expect(print.disabled).toBe(true);
    fireEvent.click(attestation);
    expect(print.disabled).toBe(false);
    await choose("장치 선택", "synthetic-two");
    expect(attestation.checked).toBe(false);
    expect(print.disabled).toBe(true);
    fireEvent.click(attestation);
    expect(print.disabled).toBe(false);
    expect(
      mocks.invoke.mock.calls.some((call) => call[0] === "start_print"),
    ).toBe(false);
  });
  it("reloads an identical settings path and replaces its fingerprint", async () => {
    render(<App />);
    await openAndSelect();
    mocks.open.mockResolvedValue("/same.json");
    let loads = 0;
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "preview_label") {
        const p = fixture();
        p.input_sha256 = `reload-${++loads}`;
        p.settings.paper.width_mm = loads === 1 ? 40 : 45;
        return Promise.resolve(p);
      }
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("설정 불러오기"));
    await screen.findByText("reload-1");
    fireEvent.click(screen.getByText("설정 불러오기"));
    await screen.findByText("reload-2");
    expect(
      (screen.getByLabelText("용지 폭 (mm)") as HTMLInputElement).value,
    ).toBe("45");
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(
      false,
    );
  });
  it("preserves pending paper and offset edits across both pattern transitions", async () => {
    render(<App />);
    await openAndSelect();
    let resolvePattern: (p: Preview) => void = () => {};
    mocks.invoke.mockImplementation(
      (
        command: string,
        args: {
          request: {
            test_pattern: boolean;
            overrides: { width_mm?: number; offset_x_mm?: number };
          };
        },
      ) => {
        if (command !== "preview_label") return Promise.resolve(null);
        if (args.request.test_pattern)
          return new Promise((resolve) => {
            resolvePattern = resolve;
          });
        const p = fixture();
        p.settings.paper.width_mm = args.request.overrides.width_mm ?? 40;
        p.settings.layout.offset_x_mm = args.request.overrides.offset_x_mm ?? 0;
        return Promise.resolve(p);
      },
    );
    fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), {
      target: { value: "45" },
    });
    fireEvent.change(screen.getByLabelText("X 보정 (mm)"), {
      target: { value: "0.5" },
    });
    fireEvent.click(screen.getByText("보정 패턴"));
    await waitFor(() =>
      expect(
        mocks.invoke.mock.calls.some(
          (c) => c[0] === "preview_label" && c[1].request.test_pattern,
        ),
      ).toBe(true),
    );
    let call = mocks.invoke.mock.calls
      .filter((c) => c[0] === "preview_label")
      .at(-1)![1].request;
    expect(call.overrides.width_mm).toBe(45);
    expect(call.overrides.offset_x_mm).toBe(0.5);
    fireEvent.click(screen.getByText("라벨 보기"));
    await screen.findByAltText(/최종 흑백/);
    call = mocks.invoke.mock.calls
      .filter((c) => c[0] === "preview_label")
      .at(-1)![1].request;
    expect(call.test_pattern).toBe(false);
    expect(call.overrides.width_mm).toBe(45);
    expect(call.overrides.offset_x_mm).toBe(0.5);
    await act(async () => resolvePattern(fixture()));
    expect(
      (screen.getByLabelText("용지 폭 (mm)") as HTMLInputElement).value,
    ).toBe("45");
  });
  it("loads an oversized source on 50×30 paper before resizing", async () => {
    const p = fixture();
    p.settings.paper = { width_mm: 50, height_mm: 30 };
    mocks.invoke.mockResolvedValue(p);
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByLabelText("용지 폭 (mm)") as HTMLInputElement).value).toBe("50");
    expect((screen.getByLabelText("용지 높이 (mm)") as HTMLInputElement).value).toBe("30");
    expect(mocks.invoke.mock.calls.find(c => c[0] === "preview_label")![1].request.overrides).toEqual({});
  });
  it("keeps invalid initial sidecars strict and retains invalid/blank overrides after reset", async () => {
    mocks.invoke.mockImplementation(
      (
        command: string,
        args: {
          request: { test_pattern: boolean; defaults: boolean; overrides: { width_mm?: number; height_mm?: number } };
        },
      ) => {
        if (command !== "preview_label") return Promise.resolve(null);
        if (!args.request.defaults)
          return Promise.reject({
            code: "invalid_settings",
            detail: "설정 파일의 용지 범위 오류: 고급 설정에서 초기화하세요.",
          });
        const p = fixture();
        p.settings.paper.width_mm = 50;
        p.settings.paper.height_mm = 80;
        return Promise.resolve(p);
      },
    );
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("용지 범위 오류"),
    );
    expect(screen.getByText("미리보기를 만들 수 없습니다")).toBeTruthy();
    for (const label of ["용지 폭 (mm)", "용지 높이 (mm)"]) expect((screen.getByLabelText(label) as HTMLInputElement).value).toBe("");
    fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), { target: { value: "50" } });
    fireEvent.change(screen.getByLabelText("용지 높이 (mm)"), { target: { value: "80" } });
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("설정 파일의 용지 범위 오류"));
    expect(screen.queryByAltText(/최종 흑백/)).toBeNull();
    fireEvent.click(screen.getByText("설정 초기화"));
    await screen.findByAltText(/최종 흑백/);
    expect(
      (screen.getByLabelText("용지 폭 (mm)") as HTMLInputElement).value,
    ).toBe("50");
    for (const value of ["51", ""]) {
      const before = mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").length;
      fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), { target: { value } });
      fireEvent.blur(screen.getByLabelText("용지 폭 (mm)"));
      expect((screen.getByLabelText("용지 폭 (mm)") as HTMLInputElement).value).toBe(value);
      expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(true);
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 180)); });
      expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label")).toHaveLength(before);
    }
    expect(mocks.invoke.mock.calls.filter(c => c[0] === "preview_label").every(c => !c[1].request.test_pattern)).toBe(true);
  });
  it("keeps out-of-range and fractional byte controls invalid until corrected", async () => {
    render(<App />);
    await openAndSelect();
    for (const [label, value] of [
      ["농도", "999"],
      ["농도", "1.5"],
      ["속도", "1.5"],
    ]) {
      const before = mocks.invoke.mock.calls.filter(
        (c) => c[0] === "preview_label",
      ).length;
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
      fireEvent.blur(screen.getByLabelText(label));
      await waitFor(() =>
        expect(screen.getByRole("status").textContent).toMatch(
          /범위.*정수|정수.*범위/,
        ),
      );
      expect((screen.getByLabelText(label) as HTMLInputElement).value).toBe(
        value,
      );
      expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(
        true,
      );
      await new Promise((resolve) => setTimeout(resolve, 180));
      expect(
        mocks.invoke.mock.calls.filter((c) => c[0] === "preview_label"),
      ).toHaveLength(before);
      fireEvent.change(screen.getByLabelText(label), {
        target: { value: "1" },
      });
      await screen.findByAltText(/최종 흑백/);
    }
    fireEvent.change(screen.getByLabelText("농도"), {
      target: { value: "999" },
    });
    fireEvent.click(screen.getByText("보정 패턴"));
    expect((screen.getByLabelText("농도") as HTMLInputElement).value).toBe(
      "999",
    );
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
  it("blank numeric input cannot serialize to a default preview or enable Print", async () => {
    render(<App />);
    await openAndSelect();
    const before = mocks.invoke.mock.calls.filter(
      (c) => c[0] === "preview_label",
    ).length;
    fireEvent.change(screen.getByLabelText("용지 폭 (mm)"), {
      target: { value: "" },
    });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("숫자 입력값"),
    );
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(
      true,
    );
    await new Promise((resolve) => setTimeout(resolve, 200));
    expect(
      mocks.invoke.mock.calls.filter((c) => c[0] === "preview_label"),
    ).toHaveLength(before);
  });
  it("prints exactly the reviewed settings, prevents duplicate starts and handles completion", async () => {
    render(<App />);
    await openAndSelect();
    let resolveStart: (v: unknown) => void = () => {};
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "start_print")
        return new Promise((resolve) => {
          resolveStart = resolve;
        });
      if (command === "get_runtime")
        return Promise.resolve({
          id: 1,
          state: "completed",
          finished: true,
          sent_bytes: 20,
          total_bytes: 20,
        });
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("1장 인쇄"));
    fireEvent.click(screen.getByText("1장 인쇄"));
    expect(
      mocks.invoke.mock.calls.filter((c) => c[0] === "start_print"),
    ).toHaveLength(1);
    const sent = mocks.invoke.mock.calls.find((c) => c[0] === "start_print")![1]
      .request;
    expect(sent).toEqual({
      label: { path: "/label.svg", test_pattern: false, settings: null, defaults: false, overrides: {}, snapshot: fixture().settings },
      device: "exact-id", model: "M110", copies: 1, expect_sha256: "raster", expect_input_sha256: "input",
    });
    expectMutationsLocked();
    expect(sent.device).toBe("exact-id");
    expect(sent.expect_sha256).toBe("raster");
    expect(sent.expect_input_sha256).toBe("input");
    expect(sent.label.snapshot).toEqual(fixture().settings);
    await act(async () =>
      resolveStart({
        id: 1,
        state: "preparing",
        finished: false,
        sent_bytes: 0,
        total_bytes: 20,
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("인쇄 완료"),
    );
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(
      false,
    );
  });
  it("discards late previous-file previews and resets overrides for new labels", async () => {
    render(<App />);
    let resolveOld: (p: Preview) => void = () => {};
    mocks.invoke.mockImplementation(
      (command: string, args: { request: { path: string } }) =>
        command === "preview_label" && args.request.path === "/label.svg"
          ? new Promise((resolve) => {
              resolveOld = resolve;
            })
          : Promise.resolve({ ...fixture(), sha256: "new" }),
    );
    fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalled());
    mocks.open.mockResolvedValue("/new.svg");
    fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByAltText(/최종 흑백/);
    await act(async () => resolveOld({ ...fixture(), sha256: "old" }));
    expect(screen.getByText("new")).toBeTruthy();
    expect(screen.queryByText("old")).toBeNull();
  });
  it("invalidates on numeric edits and on external file mismatch, then refreshes", async () => {
    render(<App />);
    await openAndSelect();
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "start_print")
        return Promise.resolve({ id: 1, state: "preparing", finished: false });
      if (command === "get_runtime")
        return Promise.resolve({
          id: 1,
          state: "failed",
          finished: true,
          error: {
            code: "hash_mismatch",
            detail: "변경된 파일: 다시 미리보기",
          },
        });
      if (command === "preview_label") return Promise.resolve(fixture());
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("1장 인쇄"));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("변경된 파일"),
    );
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(
      true,
    );
    fireEvent.click(screen.getByText("미리보기 새로고침"));
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(
      false,
    );
    fireEvent.change(screen.getByLabelText("X 보정 (mm)"), {
      target: { value: "0.5" },
    });
    expect((screen.getByText("1장 인쇄") as HTMLButtonElement).disabled).toBe(
      true,
    );
    await screen.findByAltText(/최종 흑백/);
  });
  it("pattern stays one copy and uses current settings without writing sidecars", async () => {
    render(<App />);
    await openAndSelect();
    fireEvent.change(screen.getByLabelText("인쇄 매수"), {
      target: { value: "4" },
    });
    fireEvent.click(screen.getByText("보정 패턴"));
    await screen.findByAltText(/최종 흑백/);
    expect((screen.getByLabelText("인쇄 매수") as HTMLInputElement).value).toBe(
      "1",
    );
    const call = mocks.invoke.mock.calls
      .filter((c) => c[0] === "preview_label")
      .at(-1)![1].request;
    expect(call.test_pattern).toBe(true);
    expect(call.path).toBeNull();
    expect(call.overrides.width_mm).toBe(40);
    expect((screen.getByText("설정 저장") as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
  it("fit scales long labels; dot zoom is integer", async () => {
    const p = fixture();
    p.geometry.height = 800;
    p.settings.paper.height_mm = 100;
    mocks.invoke.mockResolvedValue(p);
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    const img = await screen.findByAltText(/최종 흑백/);
    expect(parseFloat((img as HTMLImageElement).style.height)).toBeCloseTo(320);
    await choose("화면 확대", "2");
    expect((img as HTMLImageElement).style.height).toBe("1600px");
  });
  it("saves settings repeatedly and refreshes even when the same path is selected", async () => {
    render(<App />);
    await openAndSelect();
    mocks.save.mockResolvedValue("/saved.json");
    mocks.invoke.mockImplementation((command: string) =>
      Promise.resolve(
        command === "save_settings" || command === "preview_label"
          ? fixture()
          : null,
      ),
    );
    for (let n = 1; n <= 2; n++) {
      fireEvent.click(screen.getByText("설정 저장"));
      await waitFor(() =>
        expect(
          mocks.invoke.mock.calls.filter((c) => c[0] === "save_settings"),
        ).toHaveLength(n),
      );
      await screen.findByAltText(/최종 흑백/);
    }
    const saved = mocks.invoke.mock.calls
      .filter((c) => c[0] === "save_settings")
      .at(-1)![1];
    expect(saved.expectInputSha256).toBe("input");
    expect(saved.request.snapshot).toEqual(fixture().settings);
  });
  it("cancel stops the job and controls remain locked until cleanup finishes", async () => {
    render(<App />);
    await openAndSelect();
    let current = {
      id: 3,
      state: "sending",
      finished: false,
      sent_bytes: 4,
      total_bytes: 100,
    };
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "start_print" || command === "get_runtime")
        return Promise.resolve({ ...current });
      if (command === "cancel_job") {
        current = { ...current, state: "cancelled" };
        return Promise.resolve(null);
      }
      return Promise.resolve(null);
    });
    fireEvent.click(screen.getByText("1장 인쇄"));
    await waitFor(() =>
      expect((screen.getByText("취소") as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
    fireEvent.click(screen.getByText("취소"));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("cancel_job", { id: 3 }),
    );
    expect(
      (screen.getByText("이미지 열기") as HTMLButtonElement).disabled,
    ).toBe(true);
    current = { ...current, finished: true };
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("인쇄 취소"),
    );
    await waitFor(() =>
      expect(
        (screen.getByText("이미지 열기") as HTMLButtonElement).disabled,
      ).toBe(false),
    );
  });
  it("uses actual HeroUI controls, one primary action, and unframed sections", async () => {
    const { container } = render(<App />);
    expect(container.querySelectorAll(".button--primary")).toHaveLength(1);
    expect(container.querySelector(".button--primary")?.textContent).toBe("이미지 열기");
    expect(screen.getAllByText("이미지를 열거나 놓으세요")).toHaveLength(1);
    expect(container.querySelector("#print-reason")?.textContent).toBe("");
    expect(container.querySelector("[aria-live] #print-reason")).toBeTruthy();
    await openAndSelect();
    expect(container.querySelectorAll(".button--primary")).toHaveLength(1);
    expect(container.querySelector(".button--primary")?.textContent).toBe("1장 인쇄");
    expect(container.querySelectorAll(".input").length).toBeGreaterThan(2);
    expect(container.querySelector(".checkbox__control")).toBeTruthy();
    expect(container.querySelector(".card, .surface, fieldset")).toBeNull();
    expect(container.querySelectorAll(".inspector-scroll > .accordion")).toHaveLength(3);
    expect(container.querySelector("details, summary, progress, .accordion .accordion")).toBeNull();
    for (const name of ["고급 설정", "상세 정보"]) expect(screen.getByRole("button", { name }).getAttribute("aria-expanded")).toBe("false");
  });
  it("scan explicitly locks every mutable control and cannot start twice", async () => {
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    await screen.findByAltText(/최종 흑백/);
    let resolveScan: (value: unknown) => void = () => {};
    mocks.invoke.mockImplementation((command: string) => command === "scan_devices" ? new Promise((resolve) => { resolveScan = resolve; }) : Promise.resolve(fixture()));
    fireEvent.click(screen.getByText("Bluetooth 검색"));
    fireEvent.click(screen.getByText("검색 중…"));
    expect(mocks.invoke.mock.calls.filter((c) => c[0] === "scan_devices")).toHaveLength(1);
    expectMutationsLocked();
    const width = screen.getByLabelText("용지 폭 (mm)") as HTMLInputElement;
    fireEvent.change(width, { target: { value: "50" } });
    expect(width.value).toBe("40");
    expect(choice("화면 확대").disabled).toBe(false);
    await act(async () => resolveScan([]));
    expect((screen.getByText("Bluetooth 검색") as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByRole("status").textContent).toContain("검색에 나타나지 않을 수");
  });
  it.each([true, false])("discards late settings dialog success=%s after a print starts", async (success) => {
    render(<App />);
    await openAndSelect();
    const dialog = deferred<unknown>();
    mocks.open.mockReturnValue(dialog.promise);
    fireEvent.click(screen.getByText("설정 불러오기"));
    mocks.invoke.mockImplementation((command: string) => command === "start_print" ? new Promise(() => {}) : Promise.resolve(null));
    fireEvent.click(screen.getByText("1장 인쇄"));
    await act(async () => { if (success) dialog.resolve("/late.json"); else dialog.reject({ detail: "late dialog failure" }); });
    expect(screen.queryByText(/late.json/)).toBeNull();
    expect(screen.queryByText(/late dialog failure/)).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("입력 재검증 중");
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(true);
  });

});

describe("English and Korean localization", () => {
  async function language(locale: "en" | "ko") {
    const label = document.documentElement.lang === "ko" ? "언어" : "Language";
    const trigger = choice(label);
    const options = await choices(label);
    expect(screen.getAllByLabelText(label === "언어" ? "무시" : "Dismiss").length).toBeGreaterThan(0);
    fireEvent.click(options.find(option => option.getAttribute("data-key") === locale)!);
    await waitFor(() => expect(document.documentElement.lang).toBe(locale));
    await waitFor(() => expect(document.activeElement).toBe(trigger));
    expect(trigger.disabled).toBe(false);
  }
  const operations = () => mocks.invoke.mock.calls.filter(call => call[0] !== "get_runtime");

  it.each([['ko-KR', 'ko', '이미지 열기'], ['en-US', 'en', 'Open image'], ['fr-FR', 'en', 'Open image']])("uses the system locale %s with an English fallback", async (system, locale, open) => {
    localStorage.removeItem("openlabel.locale.v1");
    vi.spyOn(navigator, "language", "get").mockReturnValue(system);
    render(<App />);
    expect(screen.getByRole("button", { name: open })).toBeTruthy();
    expect(document.documentElement.lang).toBe(locale);
    expect(document.documentElement.dir).toBe("ltr");
    expect(screen.getByRole("main").lang).toBe(locale);
    expect(localStorage.getItem("openlabel.locale.v1")).toBeNull();
  });

  it("prefers and persists an explicit selection across mounts", async () => {
    vi.spyOn(navigator, "language", "get").mockReturnValue("en-US");
    const view = render(<App />);
    expect(screen.getByRole("button", { name: "이미지 열기" })).toBeTruthy();
    await language("en");
    expect(screen.getByRole("button", { name: "Open image" })).toBeTruthy();
    expect(localStorage.getItem("openlabel.locale.v1")).toBe("en");
    expect(operations()).toHaveLength(0);
    view.unmount();
    vi.spyOn(navigator, "language", "get").mockReturnValue("ko-KR");
    render(<App />);
    expect(document.documentElement.lang).toBe("en");
    expect(screen.getByRole("button", { name: "Open image" })).toBeTruthy();
  });

  it("falls back for invalid or unreadable storage and retains a selection when saving fails", async () => {
    localStorage.setItem("openlabel.locale.v1", "unsupported");
    vi.spyOn(navigator, "language", "get").mockReturnValue("ko-KR");
    const view = render(<App />);
    expect(document.documentElement.lang).toBe("ko");
    view.unmount();
    vi.spyOn(localStorage, "getItem").mockImplementation(() => { throw new Error("storage unavailable"); });
    vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    render(<App />);
    expect(document.documentElement.lang).toBe("ko");
    await language("en");
    expect(screen.getByText("Language could not be saved. It will stay selected in this window.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open image" })).toBeTruthy();
    expect(operations()).toHaveLength(0);
  });

  it("changes pending preview text, numeric validation and tooltips without re-reading or changing the preview", async () => {
    const pending = deferred<Preview>();
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "preview_label" ? pending.promise : base(command, args));
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "이미지 열기" }));
    await waitFor(() => expect(operations()).toHaveLength(1));
    const request = structuredClone(latestPreviewRequest());
    await language("en");
    expect(screen.getByText("Preparing preview")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Calculating preview");
    expect(operations()).toHaveLength(1);
    await act(async () => pending.resolve(fixture()));
    const image = await screen.findByAltText("Final monochrome print dots for a 40 × 30mm label");
    const png = image.getAttribute("src");
    expect(screen.getByText("raster")).toBeTruthy();
    expect(screen.getByText("input")).toBeTruthy();
    await language("ko");
    expect(image.getAttribute("src")).toBe(png);
    expect(image.getAttribute("alt")).toContain("최종 흑백");
    expect(latestPreviewRequest()).toEqual(request);
    expect(operations()).toHaveLength(1);
    fireEvent.change(screen.getByLabelText("농도"), { target: { value: "1.5" } });
    expect(screen.getByRole("status").textContent).toContain("정수");
    await language("en");
    expect(screen.getByLabelText("Density").getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByRole("status").textContent).toContain("set Density to an integer between 1 and 15");
    expect(screen.getByRole("button", { name: "Advanced settings" }).getAttribute("aria-expanded")).toBe("true");
    expect(operations()).toHaveLength(1);
    act(() => choice("Screen zoom").focus());
    fireEvent.focus(choice("Screen zoom"));
    expect(await screen.findByRole("tooltip")).toHaveProperty("textContent", "Screen zoom does not change the print size.");
  });

  it("translates late structured Rust errors and recovery while preserving external data", async () => {
    const pending = deferred<Preview>();
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "preview_label" ? pending.promise : base(command, args));
    render(<App />);
    fireEvent.click(screen.getByText("이미지 열기"));
    await waitFor(() => expect(operations()).toHaveLength(1));
    await language("en");
    await act(async () => pending.reject({ code: "invalid_label", detail: "Attribute not allowed: custom-속성", message: { key: "err.svgAttribute", params: { name: "custom-속성" } } }));
    expect(screen.getByRole("status").textContent).toBe("Attribute not allowed: custom-속성 · Open another image or reset in advanced settings.");
    await language("ko");
    expect(screen.getByRole("status").textContent).toBe("허용하지 않는 attribute: custom-속성 · 다른 이미지를 열거나 고급 설정에서 초기화하세요.");
    expect(operations()).toHaveLength(1);
  });

  it("changes pending, sending and terminal print text without changing requests, hashes, locks or results", async () => {
    render(<App />); await openAndSelect();
    const png = screen.getByAltText(/최종 흑백/).getAttribute("src");
    const pending = deferred<Job>();
    const runtime = runtimeFixture();
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "start_print" ? pending.promise : command === "get_runtime" ? Promise.resolve(runtime) : base(command, args));
    fireEvent.click(screen.getByText("1장 인쇄"));
    const before = structuredClone(operations());
    await language("en");
    expect(screen.getByRole("status").textContent).toContain("Revalidating input");
    expect((screen.getByLabelText("Print copies") as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Open image" }) as HTMLButtonElement).disabled).toBe(true);
    const sending = { ...jobFixture(1, "desktop", "sending", false), sent_bytes: 4 };
    runtime.job = runtime.desktop_job = sending; runtime.busy = true;
    await act(async () => pending.resolve(sending));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Sending print data"));
    expect(screen.getByRole("progressbar", { name: "Transfer progress" }).getAttribute("aria-valuenow")).toBe("4");
    await language("ko");
    expect(screen.getByRole("status").textContent).toContain("인쇄 전송 중");
    expect(screen.getByRole("progressbar", { name: "전송 진행" }).getAttribute("aria-valuenow")).toBe("4");
    expect(screen.getByAltText(/최종 흑백/).getAttribute("src")).toBe(png);
    expect(operations()).toEqual(before);
    runtime.job = runtime.desktop_job = { ...sending, state: "completed", finished: true, sent_bytes: 10 }; runtime.busy = false;
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("인쇄 완료"));
    await language("en");
    expect(screen.getByRole("status").textContent).toContain("Print complete");
    expect((screen.getByLabelText("Print copies") as HTMLInputElement).disabled).toBe(false);
    expect(screen.getByText("raster")).toBeTruthy(); expect(screen.getByText("input")).toBeTruthy();
    expect(operations()).toEqual(before);
    expect(operations().find(call => call[0] === "start_print")![1].request).toMatchObject({ device: "exact-id", expect_sha256: "raster", expect_input_sha256: "input", label: { snapshot: fixture().settings } });
    fireEvent.change(screen.getByLabelText("Print copies"), { target: { value: "2" } });
    expect(screen.getByRole("button", { name: "Print 2 copies" })).toBeTruthy();
  });

  it("passes selected language titles and filters to file dialogs", async () => {
    localStorage.setItem("openlabel.locale.v1", "en");
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Open image" }));
    await screen.findByAltText(/Final monochrome/);
    expect(mocks.open).toHaveBeenLastCalledWith(expect.objectContaining({ title: "Open image", filters: [{ name: "Images", extensions: ["svg", "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"] }] }));
    reveal(screen.getByText("Load settings"));
    mocks.open.mockResolvedValue(null);
    fireEvent.click(screen.getByText("Load settings"));
    await waitFor(() => expect(mocks.open).toHaveBeenLastCalledWith(expect.objectContaining({ title: "Load settings", filters: [{ name: "OpenLabel settings", extensions: ["json"] }] })));
    await language("ko");
    mocks.save.mockResolvedValue(null);
    fireEvent.click(screen.getByText("설정 저장"));
    await waitFor(() => expect(mocks.save).toHaveBeenLastCalledWith(expect.objectContaining({ title: "설정 저장", defaultPath: "/label.openlabel.json", filters: [{ name: "Openlabel 설정", extensions: ["json"] }] })));
  });
  it("keeps cancellation and connection errors in the current language without reconnecting or retrying", async () => {
    render(<App />); await openAndSelect();
    const runtime = runtimeFixture(), pendingCancel = deferred<void>();
    const sending = jobFixture(1, "desktop", "sending", false);
    const base = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) => command === "start_print" ? Promise.resolve(sending) : command === "cancel_job" ? pendingCancel.promise : command === "get_runtime" ? Promise.resolve(runtime) : base(command, args));
    fireEvent.click(screen.getByText("1장 인쇄"));
    await screen.findByRole("progressbar");
    fireEvent.click(screen.getByText("취소"));
    const before = structuredClone(operations());
    await language("en");
    expect(screen.getByRole("status").textContent).toContain("Cancelling remaining transmission");
    const cancelled = { ...sending, state: "cancelled", finished: false, error: { code: "cancelled", detail: "Print cancelled. Data already sent may still print.", message: { key: "err.printCancelled", params: {} } } };
    runtime.job = runtime.desktop_job = cancelled;
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Data already sent may still print"));
    await act(async () => pendingCancel.reject({ code: "late", detail: "must not replace terminal" }));
    await language("ko");
    expect(screen.getByRole("status").textContent).toContain("이미 전송된 부분은 출력될 수 있습니다");
    expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(true);
    runtime.job = runtime.desktop_job = { ...cancelled, finished: true };
    runtime.connection = { state: "disconnected", device: null, evidence: null, error: { code: "disconnected", detail: "Cannot verify the printer connection. Connect again.", message: { key: "err.connectionLost", params: {} } } };
    await waitFor(() => expect((screen.getByText("이미지 열기") as HTMLButtonElement).disabled).toBe(false));
    expect(screen.getByText("프린터 연결을 확인할 수 없습니다. 다시 연결하세요.")).toBeTruthy();
    await language("en");
    expect(screen.getByText("Cannot verify the printer connection. Connect again.")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Data already sent may still print");
    expect(operations()).toEqual(before);
  });

  it("serializes native menu updates so rapid changes finish in the latest language", async () => {
    const first = deferred<void>(), second = deferred<void>();
    mocks.menu.mockImplementationOnce(() => first.promise).mockImplementationOnce(() => second.promise);
    render(<App />);
    await waitFor(() => expect(mocks.menu).toHaveBeenCalledTimes(1));
    await language("en");
    await language("ko");
    expect(mocks.menu.mock.calls).toEqual([[{ locale: "ko" }]]);
    await act(async () => first.reject({ detail: "old menu failure" }));
    await waitFor(() => expect(mocks.menu).toHaveBeenCalledTimes(2));
    expect(mocks.menu.mock.calls[1]).toEqual([{ locale: "en" }]);
    await act(async () => second.resolve());
    await waitFor(() => expect(mocks.menu).toHaveBeenCalledTimes(3));
    expect(mocks.menu.mock.calls[2]).toEqual([{ locale: "ko" }]);
    expect(document.documentElement.lang).toBe("ko");
    expect(screen.queryByText(/old menu failure/)).toBeNull();
    expect(operations()).toHaveLength(0);
  });

  it("keeps the chosen UI language and offers recovery if native menu updates fail", async () => {
    localStorage.setItem("openlabel.locale.v1", "en");
    mocks.menu.mockRejectedValueOnce({ detail: "native menu fault" });
    render(<App />);
    expect(await screen.findByText("Menu language could not be updated. Choose another language and try again. native menu fault")).toBeTruthy();
    expect(document.documentElement.lang).toBe("en");
    await language("ko");
    await waitFor(() => expect(mocks.menu).toHaveBeenLastCalledWith({ locale: "ko" }));
    expect(screen.queryByText(/native menu fault/)).toBeNull();
    expect(operations()).toHaveLength(0);
  });

});
