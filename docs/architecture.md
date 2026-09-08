# Architecture and print contracts

OpenLabel is a Tauri desktop app and a CLI over one Rust core. Rendering is local;
there is no account, cloud upload, browser BLE, or separate background daemon.
The current target is macOS on Apple Silicon with a Phomemo M110.

## Ownership

| Layer | Responsibility |
|---|---|
| `src/App.tsx` | Display, disclosures, and preview zoom |
| `src/usePrintWorkflow.ts` | User operations, validation state, and stale-response rejection |
| `src/native.ts` | All frontend Tauri calls, dialogs, and file-drop IO |
| `src/contracts.ts` | Shared frontend DTOs |
| `src/i18n.ts`, `src/locales/*.json` | Pure message formatting and shared English/Korean catalogs |
| `src-tauri/src/commands.rs`, `bin/openlabel.rs` | Desktop and CLI adapters |
| `job.rs` | Admission, one active job, retained connection, and finalization |
| `ipc.rs` | Same-user desktop/CLI transport |
| `label.rs` | Bounded input validation, settings resolution, and render subprocess |
| `raster.rs`, `m110.rs` | Layout and final dots; protocol encoding |
| `ble.rs`, `operation.rs`, `settings.rs` | BLE admission/IO; cancellation/locks; settings/files |

Dependencies flow toward the core's lower layers. `job` calls `label`, `ble`,
`m110`, `operation`, and `settings`; `ipc` calls `job` and `operation`.
`label` calls `raster`, `settings`, and `operation`; `raster` and `m110` call
`settings`; `ble` calls `operation`. Shared errors and hashing live in `lib.rs`.
The core cannot import desktop/CLI adapters. The TypeScript/CSS guard and Rust
architecture tests reject cycles, forbidden edges, and unclassified production files.

## Localization

The workflow stores stable message keys and structured errors; App formats them
in the selected locale. Locale storage is separate from paper/settings, and
locale is absent from preview, print, connection, and polling effect dependencies.
A separate serialized `set_ui_locale` call updates only the default macOS menus;
its Tauri entry validates en/ko and preserves predefined roles and shortcuts.
`native.ts` passes the current locale's titles and filters to file dialogs.
The catalogs are leaves; Rust includes the English catalog for diagnostics.

Errors retain `code` and `detail` and may add `message: {key, params, cause?}`.
`params` maps names to literal strings; optional `cause` is another message for
contextual recovery hints. App-owned constructors supply stable keys, while
external library/OS errors retain their original text. CLI diagnostics default
to English; the frontend translates metadata at render time and falls back to
`detail` for old errors or unknown keys. JSON version, request schemas, settings,
PNG contents, and both hash contracts are unchanged.

## Input and rendering

Formats are detected from bytes. PNG, JPEG, BMP, GIF, WebP, and TIFF accept files
up to 16 MiB, axes up to 16,384 pixels, and at most 16 million pixels.
Decoder allocation is bounded at 128 MiB. GIF/WebP use the first frame and TIFF
the first page; decoder-provided EXIF orientation is applied. Transparency is
composited onto white. Associated-alpha TIFF is normalized before composition.
WebP container, chunk, frame, and bitstream dimensions are checked before decoding.

SVG is UTF-8, at most 1 MiB, with the SVG namespace and a valid size or viewBox.
Allowed elements are `svg` (root only), `g`, `text`, `tspan`, `path`, `rect`,
`circle`, `line`, and `image`. Only bounded presentation attributes/styles are
accepted. Scripts, DTD/entities, processing instructions, external resources,
CSS resource references, `use`, and filters are rejected.
Inline images must be base64 PNG/JPEG, totaling at most 512 KiB and 4 million pixels.
SVG is limited to 4,096 elements, 32 element levels, 128 KiB of path data,
100,000 px source axes, and viewBox values within ±1,000,000.
Use `Nanum Gothic`, `NanumGothic`, or `sans-serif`; all resolve to the bundled
NanumGothic Regular. No system fonts or remote fonts are loaded.

Desktop and CLI rendering runs in a subprocess with a five-second deadline;
timeout/cancellation kills and reaps it. One render per process is admitted at
once. A print waits up to eight seconds for render contention, then fails.
The public CLI adapter never renders SVG as shell commands.

## Settings and coordinates

Settings resolve as defaults → adjacent sidecar (or explicit replacement) →
snapshot, when supplied → per-request overrides. A present sidecar is validated
even when a snapshot follows it. Invalid settings never silently become defaults.
See [CLI settings](agent-cli.md#settings) for naming and export.

| Setting | Default and range |
|---|---|
| Paper | Valid explicit SVG mm size; otherwise 50×30 mm; width 20–50, height 10–100 mm |
| Artwork scale | 100% contain fit; 10–200%; calibration pattern fixed at 100% |
| Alignment / margin | `center` (`left`, `center`, `right`); 1 mm, range 0–5 mm with positive content area |
| Mirror / rotation | false / 0; mirror first, then clockwise 0/90/180/270° |
| X/Y offset | 0 mm; each −10 to +10 mm in physical printer axes |
| Raster | `threshold` at 128, or `floyd-steinberg` (threshold normalized to 128) |
| White cutoff | 255; integer 0–255 |
| Density / speed | 10 / 1; integer ranges 1–15 / 1–5 |
| Copies | Per job, 1–10; calibration pattern always one |

The calibration pattern defaults to 40×30 mm. Software input ranges are not a
hardware compatibility claim. Desktop paper persistence is described in the
[design rules](design-system.md).

Coordinates start at the top left; +X is right and +Y follows paper feed.
Dimensions use `floor(mm * 8 + 0.5)`; signed offsets round magnitude then restore
sign. For paper width W and height H in dots, transmission is always 384×H,
48 bytes per row. Paper X origin is 0, `floor((384-W)/2)`, or `384-W` for
left/center/right alignment. Printable area is the paper/head intersection;
content also excludes margins. Gap length is not added to H.

Artwork uses aspect-preserving contain fit, then scale. Rotation changes artwork,
not paper dimensions or calibration axes. Offset is applied after rotation;
anything outside the paper/head intersection is clipped. At 50 mm centered paper,
1 mm on each edge lies outside the 48 mm head. `clipped_dot_count` counts black
dots lost during final placement, not pixels already cropped by artwork scaling.

The renderer packs monochrome dots MSB first, 1 for black. The preview PNG is
reconstructed from these exact bits; the encoder does not transform them again.
Paper boundaries and unprintable stripes are display overlays. Screen millimeters
and thermal density are not calibrated representations of physical output.

## Identity and admission

`sha256` hashes the versioned, length-prefixed normalized raster settings and
packed dots. It excludes density, speed, and copies. `input_sha256` hashes the
versioned, length-delimited original source bytes and sidecar bytes (or explicit
absence), excluding paths. `source_sha256` binds a sidecar to its source.
Changing sidecar formatting can change input identity without changing the raster.
Per-command density/speed overrides are validated separately; they do not change
the input hash unless written into a sidecar.

Before sending, Rust resolves and renders again and verifies both expected
hashes. The selected exact device ID, explicit M110 model attestation, copy count,
and effective settings remain required. A scan name is only a candidate hint.
BLE admission requires FF00/FF02, a write property, and MTU ≥131 for 128-byte payloads.
Write Without Response is preferred, with Write With Response as the fallback.
Known conflicting model names are rejected.

On macOS, the exact identifier is retrieved first; only an absent result or
explicitly unsupported retrieval permits one scan fallback. After connection,
a separate non-scanning adapter checks the native name for the same ID, and the
transport adapter's available name is also checked. Names do not authenticate
hardware; body-model confirmation is still required. Permission, cancellation,
timeout, and operational errors do not trigger discovery or transmission retries.

## Jobs and local IPC

A same-user OS file lock excludes overlapping desktop and transient CLI work.
The app retains its connection and lock after successful transmission; explicit
Connect sends zero characteristic bytes. Another ID requires explicit disconnect.
Each new print rechecks the actual OS link; a stale same-ID link may reconnect
for that new request. Failed transmission is never retried automatically.

The desktop hosts a versioned, length-prefixed JSON protocol on a same-user Unix
socket (Windows code uses a named pipe). Namespace ownership, permissions, peer
identity, and frame sizes are checked. Requests are limited to 64 KiB and responses
to 8 MiB. The CLI falls back to a transient job only when app absence is proven
before submission. After any request-write attempt, losing the result produces
`unknown_outcome`, with no resubmission or fallback.

Jobs progress from `preparing` to `sending`, then `completed`, `failed`, or
`cancelled`. Normal transmission plus all pacing delays means `completed`;
there is no completion ACK or post-print confirmation requirement. Terminal
results are immutable. `finished:true` means finalization is done; the next job
and CLI process completion wait for it. Cancellation stops remaining writes and
cleans up; already transmitted content may print. A job has a 180-second bound,
native operations 12 seconds, and connection cleanup two seconds. The app keeps
a successful connection; transient jobs, failed sends, cancellation, and app
shutdown perform bounded cleanup.

## M110 protocol and attribution

The encoder sends speed (`1B 4E 0D n`), density (`1B 4E 04 n`), and gap media
(`1F 11 0A`), waiting 30 ms after each. Raster header is
`1D 76 30 00 30 00 <rows:u16le>`, followed by 128-byte chunks with 20 ms delays,
then a 300 ms wait and footer `1F F0 05 00 1F F0 03 00` with a 500 ms wait.
Rows are bounded at 800. Each copy repeats the sequence.

This is an independent implementation based on protocol observations from
[MozgAI/sticker-mac at 43f5c22cff12bbdf7c0c043dc5417c3ce2ad5251](https://github.com/MozgAI/sticker-mac/tree/43f5c22cff12bbdf7c0c043dc5417c3ce2ad5251).
The [protocol fixture](../fixtures/protocol/m110-v1.json) is constructed from
those observations, **not a hardware capture**. Other advertised models and
transport families are not supported by this encoder.

## Building and checks

The macOS deployment target is 13.3; that minimum version is not independently
verified. Windows, Linux, and Intel Macs are unverified. The configured app bundle
uses ad-hoc signing, not a Developer ID signature or notarization.

Run from the repository root after `npm ci`:

```sh
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
npm run tauri -- build --no-bundle
```

`npm run build` creates frontend assets; Tauri builds the desktop executable.
`--no-bundle` skips app packaging. Default Rust outputs are in `src-tauri/target`;
if `CARGO_TARGET_DIR` is set, substitute that directory in executable/bundle paths.
The tests use fixtures and synthetic transports and do not establish hardware
or native UI compatibility. Dependency versions are recorded in both lockfiles.
