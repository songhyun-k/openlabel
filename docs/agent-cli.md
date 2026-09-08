# CLI and agent use

Build instructions are in the [README](../README.md). `openlabel --help` and
`openlabel <command> --help` describe the installed executable. All commands,
including help, return one JSON object on stdout; progress goes to stderr.
`--json` is optional and documents the caller's intent.

```json
{"version":1,"ok":true,"result":{}}
```

Errors use `{"version":1,"ok":false,"error":{"code":"…","detail":"…"}}`.
App-owned errors may also include `message: {key, params, cause?}` for GUI
translation; callers must accept this optional metadata and must not parse
`detail` sentences. Diagnostics and help default to English. JSON version 1,
error codes, requests, settings, and hashes are independent of GUI language.
Exit status is 0 for success, 2 for `invalid_arguments`, and 1 for other errors.
Wrappers should spawn the executable with an argv array, not construct a shell string.

## Commands

| Command | Required arguments | Effect |
|---|---|---|
| `devices` | none | Four-second BLE discovery; IDs/names are candidate hints |
| `check-device` | `--device ID --model M110` | Connect, validate GATT/MTU, disconnect; zero printer bytes |
| `connection` | none | Query app runtime; `app_running:false` if absent |
| `connect` | `--device ID --model M110` | Retain an app connection; zero characteristic bytes |
| `disconnect` | `--device ID` | Release that app connection |
| `preview` | image path or `--test-pattern`, `--output PATH` | Write final 1-bit PNG and return hashes/settings/geometry |
| `print` | image path, target, both expected hashes | Print 1–10 copies (`--copies`, default 1) |
| `test-print` | `--device ID --model M110` | Print one built-in calibration pattern |

`connect`/`disconnect` require the desktop app (`app_required` otherwise).
The app retains its print lock while connected, so `check-device` can return
`job_busy`; use `connection` to query it. Disconnect before choosing another ID.
The CLI submits prints to the app when present. Only proven app absence before
submission permits a transient connection. `unknown_outcome` means submission
may have occurred: inspect app state and physical output before any new request.
Never automatically retry it.

## Preview, approve, print

The following assumes `openlabel` is on PATH. These preview commands do not use BLE:

```sh
openlabel preview label.svg --width-mm 40 --height-mm 30 \
  --offset-x-mm 0.5 --settings-out label.openlabel.json \
  --output label-preview.png --json
openlabel preview photo.png --scale-percent 75.5 \
  --settings-out photo.png.openlabel-image.json --output photo-preview.png --json
```

Review the PNG, effective paper size, clipping, and copies. Select the exact ID
from `devices` and confirm the model on the printer body. An agent may print
only within the user's approved device, M110 model, paper, copies, and reviewed
preview. Existing explicit approval for that same scope need not be requested
again. Preview/discovery/connection requests alone do not authorize printing.

```sh
openlabel print label.svg --settings label.openlabel.json \
  --device 'SELECTED_DEVICE_ID' --model M110 --copies 1 \
  --expect-sha256 'PREVIEW_SHA256' --expect-input-sha256 'INPUT_SHA256' --json
```

Replace placeholders with the chosen ID and the preview response's `result.sha256`
and `result.input_sha256`. Use identical source and settings. Rust re-renders and
checks both hashes before sending; changes require a new preview and review.
`--settings-out` returns hashes after reloading the exported file, so its response
can be used directly with that file as `--settings`.

For calibration, preview first, then use the same options and both hashes after approval:

```sh
openlabel preview --test-pattern --width-mm 40 --height-mm 30 --output pattern.png
openlabel test-print --device 'SELECTED_DEVICE_ID' --model M110 \
  --width-mm 40 --height-mm 30 \
  --expect-sha256 'PREVIEW_SHA256' --expect-input-sha256 'INPUT_SHA256'
```

Pattern hashes are optional in the executable, but agents should pass the reviewed
ones. Patterns are always one copy at 100% artwork scale and have no source sidecar.

## Settings

SVG filenames use `<stem>.openlabel.json`; other filenames use
`<whole-filename>.openlabel-image.json`, such as `photo.png.openlabel-image.json`.
The naming rule follows the extension, independent of detected image format.
An adjacent sidecar loads automatically. `--settings PATH` replaces that choice;
individual options override its values. `--defaults` ignores the sidecar and
cannot be combined with `--settings`. See [ranges and defaults](architecture.md#settings-and-coordinates).

Settings JSON stores version 1, source SHA-256, paper, layout, raster, and printer
settings. Device IDs and copies are per job and are not saved. Unknown fields,
non-finite numbers, files over 64 KiB, invalid ranges, and source mismatches fail.
Use preview's `--settings-out PATH` to write a complete valid settings file.
Output files are not overwritten unless `--overwrite` is explicit; source files
are protected against replacement, including aliases such as hard links.

To hand off between CLI and desktop, transfer the source and exported settings.
The desktop remembers paper size and applies it when opening a new image. Use
**Load settings / 설정 불러오기** to adopt the exported settings' paper, then review the new PNG.
To return to CLI, save the desktop's effective settings and pass `--settings`.
Moving unchanged source/settings files does not change either hash.

## Completion and errors

The foreground command waits for `finished:true`. Normal transmission and pacing
finishing means `completed` / print complete; no completion ACK or post-print
confirmation is added. SIGINT/SIGTERM requests cancellation; shared app calls send
one cancel request and wait up to ten seconds for cleanup. Already sent content
may print. Failed or cancelled jobs are never automatically retransmitted.

Use `error.code` for handling and `error.detail` for a next action. Typical codes
include `invalid_label`, `invalid_settings`, `settings_source_mismatch`,
`output_exists`, `hash_mismatch`, `render_busy`, `render_timeout`, `job_busy`,
`permission_required`, `adapter_off`, `device_not_found`, `unsupported_device`,
`disconnected`, `transport_timeout`, `cancelled`, `app_required`, and `unknown_outcome`.
For a stale source or settings file, make a fresh preview; for permission errors,
let the user resolve macOS permissions. Do not turn failures into retry loops.
