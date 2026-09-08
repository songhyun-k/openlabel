---
name: openlabel
description: Control a Phomemo label printer through the OpenLabel CLI. Use for printer discovery and connection, image previews, print settings, and user-authorized label printing, including handoff to the desktop app.
---

# OpenLabel

Use OpenLabel's CLI for printer operations. It shares the desktop app's renderer
and print controls. The current printer implementation supports Phomemo M110;
do not infer support for another model from its advertised name.

## Find the interface

Use the executable supplied by the user or `openlabel` on PATH. In a source
checkout, the default release binary is `src-tauri/target/release/openlabel`;
respect a configured build directory. If it is unavailable, resolve installation
or the executable location before attempting printer operations.

Read `--help` and the relevant command's help for the installed version.
Commands return one JSON object on stdout: `version`, `ok`, and `result` or
`error`. Progress goes to stderr. Handle `error.code`; do not parse the wording
or language of `error.detail`. Pass arguments as an argv array when using a
process API.

## Preview and print

Use the user's content, image, paper, placement, and copy count. Sample sizes,
filenames, printer IDs, and calibration values are examples, not defaults
to impose on other jobs. Use an existing supported image when available; when
creating one, validate it through the CLI preview before printing. A preview-only
request needs no printer discovery or connection.

Inputs can be SVG, PNG, JPEG, WebP, BMP, GIF, or TIFF. For generated SVG, use
simple shapes/text and the bundled Nanum Gothic font; external resources and
filters are unsupported. Rasterize designs that need other fonts or effects
before passing them to OpenLabel.

1. For a print or connection request, resolve the printer from its exact ID or
   `devices`. Confirm the
   supported model and loaded paper from the user's context; ask only when the
   target or physical settings are ambiguous. Discovery names are candidate hints.
2. Preview the source with the intended settings. Review the generated PNG,
   paper size, orientation, and clipping. Keep both `result.sha256` and
   `result.input_sha256` from that response.
3. Print within the user's authorization for that label, device, paper, and
   copies. An explicit print request can supply that authorization; do not ask
   again when the same scope is already clear. A preview or connection request
   alone does not authorize printing.
4. Pass both expected hashes and the same source/settings to `print`. Changed
   source or settings require a new preview. Report the returned outcome.

```sh
openlabel preview INPUT_IMAGE --settings-out SETTINGS.json --output PREVIEW.png --json
openlabel print INPUT_IMAGE --settings SETTINGS.json \
  --device DEVICE_ID --model M110 --copies COPIES \
  --expect-sha256 PREVIEW_SHA256 --expect-input-sha256 INPUT_SHA256 --json
```

Use fresh output paths unless replacing an existing output is intended; replacement
requires `--overwrite`. `--settings-out` returns hashes after reloading the exported
settings, so use that file with `--settings`. Adjacent sidecars load automatically:
`<stem>.openlabel.json` for SVG, `<full-filename>.openlabel-image.json` otherwise.
`--defaults` ignores the sidecar and cannot be combined with `--settings`.
For a desktop handoff, provide the source and settings file; explicitly load the
settings when its paper size should replace the app's remembered paper.

Use `preview --test-pattern` before an authorized calibration print. `test-print`
prints one pattern; pass the reviewed hashes and the same calibration settings.

## Connection and outcomes

`connection` reports the desktop app's connection and job state. `connect` and
`disconnect` retain or release its connection. CLI prints share a running app's
jobs; when no app is present, the CLI manages a transient connection. A retained
connection to another ID must be released before choosing a different printer.
`check-device` is a transient connection check, not a status query, and may return
`job_busy` while the app holds its connection. Connecting does not print a label.

`completed` means transmission and its required pacing finished. Cancellation
stops remaining writes; already transmitted content may still print.
On `unknown_outcome`, inspect the app state and physical output before another
print. Do not automatically resend, switch transport paths, or treat a lost
response as proof that nothing printed. Resolve stale input with a new preview;
let the user handle OS permission prompts when needed.
