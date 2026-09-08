export type LocalizedMessage = {
  key: string;
  params: Record<string, string>;
  cause?: LocalizedMessage;
};
export type AppError = { code: string; detail: string; message?: LocalizedMessage };
export type Settings = {
  version: number;
  source_sha256: string;
  paper: { width_mm: number; height_mm: number };
  layout: {
    scale_percent?: number;
    alignment: string;
    margin_mm: number;
    rotation_deg: number;
    mirror: boolean;
    offset_x_mm: number;
    offset_y_mm: number;
  };
  raster: { mode: string; threshold: number; white_cutoff: number };
  printer: { density: number; speed: number };
};
export type Overrides = Partial<{
  scale_percent: number;
  width_mm: number; height_mm: number; alignment: string; margin_mm: number;
  rotation_deg: number; mirror: boolean; offset_x_mm: number; offset_y_mm: number;
  raster_mode: string; threshold: number; white_cutoff: number; density: number; speed: number;
}>;
export type Preview = {
  sha256: string;
  input_sha256: string;
  source_sha256: string;
  settings: Settings;
  settings_path: string | null;
  png_base64: string;
  clipped_dot_count: number;
  geometry: {
    paper_x: number;
    paper_width: number;
    height: number;
    head_width: number;
    printable_x: number;
    printable_width: number;
    content_x: number;
    content_y: number;
    content_width: number;
    content_height: number;
  };
};
export type Device = {
  id: string;
  name: string | null;
  rssi: number | null;
  supported: boolean;
  model_allowed: boolean;
  evidence: string;
  advertised_services: string[];
};
export type Job = {
  id: number;
  origin: string;
  state: string;
  finished: boolean;
  sent_bytes: number;
  total_bytes: number;
  copies: number; device: string; sha256: string; input_sha256: string;
  settings: Settings | null; evidence: Evidence | null;
  error: AppError | null;
};
export type ConnectionStatus = {
  state: string; device: string | null; evidence: Evidence | null;
  error: AppError | null;
};
export type Runtime = {
  app_running: boolean; revision: number; connection: ConnectionStatus;
  job: Job | null; desktop_job: Job | null; busy: boolean; closing: boolean;
};
export type Evidence = {
  device_id: string; model: string; service: string; characteristic: string;
  write_type: string; mtu: number;
};
export type LabelRequest = {
  path: string | null; test_pattern: boolean; settings: string | null;
  defaults: boolean; overrides: Overrides; snapshot: Settings | null;
};
export type PrintRequest = {
  label: LabelRequest; device: string; model: string; copies: number;
  expect_sha256: string; expect_input_sha256: string;
};
