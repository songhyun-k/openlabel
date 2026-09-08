import { useEffect, useRef, useState } from "react";
import { I18nProvider, Accordion, Button, Checkbox, Input, Label, ListBox, ProgressBar, Select, Separator, Spinner, Tooltip } from "@heroui/react";
import { translate, translateError, translateNotice, type MessageKey } from "./i18n";
import { numberFields, usePrintWorkflow } from "./usePrintWorkflow";

function Choice({ label, value, options, disabled = false, onChange, hint, placeholder = label }: { label: string; value: string | null; options: { id: string; text: string; disabled?: boolean; lang?: string }[]; disabled?: boolean; onChange: (value: string) => void; hint?: string; placeholder?: string }) {
  const trigger = <Select.Trigger aria-label={label}><Select.Value lang={options.find(o => o.id === value)?.lang}>{options.find(o => o.id === value)?.text ?? placeholder}</Select.Value><Select.Indicator/></Select.Trigger>;
  return <Select variant="secondary" value={value} onChange={key => { if (key !== null) onChange(String(key)); }} isDisabled={disabled} disabledKeys={options.filter(o => o.disabled).map(o => o.id)} placeholder={placeholder}>
    <Label>{label}</Label>
    {hint ? <Tooltip>{trigger}<Tooltip.Content>{hint}</Tooltip.Content></Tooltip> : trigger}
    <Select.Popover><ListBox>{options.map(o => <ListBox.Item key={o.id} id={o.id} textValue={o.text} lang={o.lang}>{o.text}<ListBox.ItemIndicator/></ListBox.Item>)}</ListBox></Select.Popover>
  </Select>;
}

export default function App() {
  const { locale, menuError, localeStorageFailed, selectLocale, emptyHint, path, pattern, overrides, preview, settings, paperStorageFailed, statusDetail: statusNotice, rendering, refreshRequired, effectiveScale, previewError, devices, device, attested, copies, job, canCancel, locked, scanning, invalidNumber, numericError, ready, hasLabel, reason, disabled, connection, connectionError, canConnect, connect, disconnect, change, selectDevice, attest, changeCopies, pickLabel, loadSettings, saveSettings, scan, print, cancel, togglePattern, resetSettings, refresh } = usePrintWorkflow();
  const t = (key: MessageKey, params?: Record<string, string | number>) => translate(locale, key, params);
  const statusDetail = translateNotice(locale, statusNotice);
  const status = statusDetail.length > 140 ? t("statusLong") : statusDetail;
  useEffect(() => {
    document.documentElement.lang = locale;
    document.documentElement.dir = "ltr";
  }, [locale]);
  const [zoom, setZoom] = useState("fit"), [headView, setHeadView] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const invalidField = invalidNumber?.[0];
  useEffect(() => {
    if (invalidField && !["width_mm", "height_mm", "scale_percent"].includes(invalidField)) setAdvancedOpen(true);
  }, [invalidField]);
  const panel = useRef<HTMLDivElement>(null);
  const [availableWidth, setAvailableWidth] = useState(400), [availableHeight, setAvailableHeight] = useState(320);
  useEffect(() => {
    if (!panel.current) return;
    const observer = new ResizeObserver(([entry]) => {
      setAvailableWidth(Math.max(100, entry.contentRect.width));
      setAvailableHeight(Math.max(100, entry.contentRect.height));
    });
    observer.observe(panel.current);
    return () => observer.disconnect();
  }, [!!preview]);
  function number(key: keyof typeof numberFields, value: number | undefined) {
    const [labelKey, min, max, step] = numberFields[key];
    const label = t(labelKey);
    const normalizedThreshold =
      key === "threshold" &&
      (overrides.raster_mode ?? settings?.raster.mode) === "floyd-steinberg";
    const inputValue = key === "scale_percent" ? effectiveScale : normalizedThreshold
      ? 128
      : ((overrides[key] as number) ?? value);
    return (
      <div className="field">
        <Label htmlFor={key}>{({ width_mm: t("widthShort"), height_mm: t("heightShort"), scale_percent: t("scaleShort") } as Record<string, string>)[key] ?? label}</Label>
        <Input
          id={key}
          aria-label={label}
          variant="secondary"
          aria-invalid={invalidNumber?.[0] === key}
          aria-describedby={invalidNumber?.[0] === key ? "print-reason" : undefined}
          type="number"
          value={Number.isFinite(inputValue) ? inputValue : ""}
          disabled={locked || scanning || normalizedThreshold || (key === "scale_percent" && pattern)}
          min={min}
          max={max}
          step={step}
          onChange={(e) => change(key, e.target.valueAsNumber)}
        />
      </div>
    );
  }
  const g = preview?.geometry,
    left = g ? (headView ? Math.min(0, g.paper_x) : g.paper_x) : 0,
    viewWidth = g
      ? headView
        ? Math.max(384, g.paper_x + g.paper_width) - left
        : g.paper_width
      : 384,
    zoomScale =
      zoom === "fit"
        ? Math.min(availableWidth / viewWidth, availableHeight / (g?.height ?? 240))
        : Number(zoom);
  return (
    <I18nProvider locale={locale === "ko" ? "ko-KR" : "en-US"}><main lang={locale} dir="ltr">
      <header className="page-header">
        <h1>OpenLabel</h1>
        <Button variant={hasLabel ? "secondary" : "primary"} isDisabled={disabled} onPress={pickLabel}>{t("openImage")}</Button>
        <span className="filename">{pattern ? t("pattern") : path?.split(/[\\/]/).pop() ?? ""}</span>
        <div className="language-control"><Choice label={t("language")} value={locale} onChange={selectLocale} options={[{ id: "en", text: t("languageEnglish"), lang: "en" }, { id: "ko", text: t("languageKorean"), lang: "ko" }]}/></div>
      </header>
      {localeStorageFailed && <p className="warning" role="status">{t("languageStorageFailed")}</p>}
      {menuError && <p className="warning" role="status">{translateNotice(locale, menuError)}</p>}
      <div className="workspace">
        <section aria-label={t("previewRegion")} className="preview-panel">
          <div className="preview-stage">
            {preview && g ? (
              <div ref={panel} className={`preview-scroll ${zoom === "fit" ? "fit" : ""}`}>
                <div className="paper-scene" style={{ width: viewWidth * zoomScale, height: g.height * zoomScale }}>
                  <div className="paper" style={{ left: (g.paper_x - left) * zoomScale, width: g.paper_width * zoomScale, height: g.height * zoomScale }}/>
                  <div className="head-image" style={{ left: (g.printable_x - left) * zoomScale, width: g.printable_width * zoomScale, height: g.height * zoomScale }}>
                    <img alt={t("previewAlt", { width: settings?.paper.width_mm ?? "", height: settings?.paper.height_mm ?? "" })} src={`data:image/png;base64,${preview.png_base64}`} style={{ width: 384 * zoomScale, height: g.height * zoomScale, left: -g.printable_x * zoomScale }}/>
                  </div>
                </div>
              </div>
            ) : (
              <div className="empty">{rendering && <Spinner aria-hidden="true"/>}<h2>{rendering ? t("previewPreparing") : previewError || numericError ? t("previewUnavailable") : hasLabel ? t("previewRefreshNeeded") : t("openOrDrop")}</h2>{refreshRequired && <Button variant="secondary" isDisabled={disabled} onPress={refresh}>{t("refreshPreview")}</Button>}</div>
            )}
          </div>
          <div className="view-controls">
            <Choice label={t("zoom")} value={zoom} onChange={setZoom} options={[{ id: "fit", text: t("fitScreen") }, { id: "1", text: "1 dot = 1 px" }, { id: "2", text: t("zoomTwo") }, { id: "4", text: t("zoomFour") }]} hint={t("zoomHint")}/>
          </div>
        </section>
        <aside className="inspector" aria-label={t("printSettings")}>
          <div className="inspector-scroll">
          <Accordion className="printer"><Accordion.Item id="printer"><Accordion.Heading><Accordion.Trigger><span><span>{t("printer")} · {({ connected: t("connected"), connecting: t("connecting"), disconnecting: t("disconnecting"), unknown: t("connectionUnknown") } as Record<string, string>)[connection.state] ?? t("disconnected")}</span>{device && <span className="printer-name">{devices.find(d => d.id === device)?.name ?? t("unnamed")}{!attested ? ` · ${t("checkBody")}` : ""}</span>}</span><Accordion.Indicator/></Accordion.Trigger></Accordion.Heading><Accordion.Panel><Accordion.Body>
            <Button variant="secondary" isDisabled={disabled} onPress={scan}>{scanning && <Spinner aria-hidden="true" size="sm"/>}{scanning ? t("scanning") : t("scanBluetooth")}</Button>
            <Choice placeholder={t("selectPrinter")} label={t("selectDevice")} disabled={disabled} value={device || null} onChange={selectDevice} options={devices.map(d => ({ id: d.id, text: `${d.name ?? t("unnamed")} · ${d.id} · ${d.rssi ?? "?"} dBm (${connection.evidence?.device_id === d.id ? t("verifiedConnection") : t("candidate")})`, disabled: !d.model_allowed || (!!connection.device && d.id !== connection.device) }))}/>
            <Checkbox variant="secondary" isDisabled={disabled} isSelected={attested} onChange={attest}><Checkbox.Content><Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>{t("attestModel")}</Checkbox.Content></Checkbox>
            <div className="settings-actions"><Button variant="secondary" isDisabled={!canConnect || connection.state === "connected"} onPress={connect}>{t("connect")}</Button><Button variant="ghost" isDisabled={disabled || !connection.device || !!connectionError} onPress={disconnect}>{t("disconnect")}</Button></div>
            {(connectionError || connection.error) && <p className="warning">{connectionError ? translateNotice(locale, connectionError) : translateError(locale, connection.error)}</p>}
          </Accordion.Body></Accordion.Panel></Accordion.Item></Accordion>
          <section aria-labelledby="paper-title">
            <h2 id="paper-title">{t("label")}</h2>
            <div className="settings-grid">
              {number("width_mm", settings?.paper.width_mm)}{number("height_mm", settings?.paper.height_mm)}
            </div>
            {paperStorageFailed && <p className="warning">{t("paperStorageFailed")}</p>}
          </section>
          <Separator/>
          <section aria-labelledby="image-title"><h2 id="image-title">{t("dialogImages")}</h2>
            <div className="settings-grid">
              {number("scale_percent", effectiveScale)}<Tooltip><Button variant="secondary" isDisabled={disabled || !hasLabel || pattern} onPress={() => change("scale_percent", 100)}>{t("fitImage")}</Button><Tooltip.Content>{t("fitImageHint")}</Tooltip.Content></Tooltip>
            </div>
            {settings && <div className="settings-grid">
              <Choice label={t("rotation")} disabled={disabled} value={String(overrides.rotation_deg ?? settings.layout.rotation_deg)} onChange={value => change("rotation_deg", Number(value))} options={[0, 90, 180, 270].map(value => ({ id: String(value), text: `${value}°` }))} hint={t("rotationHint")}/>
              <Checkbox variant="secondary" isDisabled={disabled} isSelected={Boolean(overrides.mirror ?? settings.layout.mirror)} onChange={(value) => change("mirror", value)}><Checkbox.Content><Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>{t("mirror")}</Checkbox.Content></Checkbox>
            </div>}
          </section>
          <Accordion className="advanced" expandedKeys={advancedOpen ? ["advanced"] : []} onExpandedChange={keys => setAdvancedOpen(keys.has("advanced"))}><Accordion.Item id="advanced"><Accordion.Heading><Accordion.Trigger>{t("advanced")}<Accordion.Indicator/></Accordion.Trigger></Accordion.Heading><Accordion.Panel><Accordion.Body>
            {settings && <>
            <section className="control-group" aria-labelledby="image-output-title"><h3 id="image-output-title">{t("imageOutput")}</h3><div className="settings-grid">
              <div className="wide"><Choice label={t("rasterMode")} disabled={disabled} value={String(overrides.raster_mode ?? settings.raster.mode)} onChange={value => change("raster_mode", value)} options={[{ id: "threshold", text: t("threshold") }, { id: "floyd-steinberg", text: "Floyd–Steinberg" }]}/></div>
              {number("threshold", settings.raster.threshold)}{number("white_cutoff", settings.raster.white_cutoff)}
            </div></section>
            <section className="control-group" aria-labelledby="position-title"><h3 id="position-title">{t("position")}</h3><div className="settings-grid">
              <Choice label={t("alignment")} disabled={disabled} value={String(overrides.alignment ?? settings.layout.alignment)} onChange={value => change("alignment", value)} options={[{ id: "left", text: t("left") }, { id: "center", text: t("center") }, { id: "right", text: t("right") }]}/>
              {number("margin_mm", settings.layout.margin_mm)}
              {number("offset_x_mm", settings.layout.offset_x_mm)}{number("offset_y_mm", settings.layout.offset_y_mm)}
              <div className="nudge">{([["←", "offset_x_mm", -0.5], ["→", "offset_x_mm", 0.5], ["↑", "offset_y_mm", -0.5], ["↓", "offset_y_mm", 0.5]] as const).map(([text, key, delta]) => <Button key={text} variant="secondary" isDisabled={disabled} aria-label={t("nudge", { direction: t(({ "←": "left", "→": "right", "↑": "up", "↓": "down" } as const)[text]) })} onPress={() => change(key, Number(overrides[key] ?? settings.layout[key]) + delta)}>{text} 0.5</Button>)}</div>
            </div></section>
            <section className="control-group" aria-labelledby="printer-output-title"><h3 id="printer-output-title">{t("densitySpeed")}</h3><div className="settings-grid">
              {number("density", settings.printer.density)}{number("speed", settings.printer.speed)}
            </div></section>
            </>}
            <section className="control-group" aria-labelledby="settings-files-title"><h3 id="settings-files-title">{t("settingsFiles")}</h3>
            <div className="settings-actions"><Button variant="secondary" isDisabled={disabled || !path || pattern} onPress={loadSettings}>{t("loadSettings")}</Button><Button variant="secondary" isDisabled={disabled || !preview || pattern || !path} onPress={saveSettings}>{t("saveSettings")}</Button><Button variant="ghost" isDisabled={disabled || !hasLabel} onPress={resetSettings}>{t("resetSettings")}</Button></div>
            </section>
            <section className="control-group" aria-labelledby="preview-tools-title"><h3 id="preview-tools-title">{t("previewTools")}</h3>
            <div className="settings-actions"><Button variant="ghost" isDisabled={disabled} aria-pressed={pattern} onPress={togglePattern}>{pattern ? t("showLabel") : t("pattern")}</Button>{!refreshRequired && <Button variant="ghost" isDisabled={disabled || !hasLabel} onPress={refresh}>{t("refreshPreview")}</Button>}</div>
            </section>
          </Accordion.Body></Accordion.Panel></Accordion.Item></Accordion>
          <Accordion className="technical"><Accordion.Item id="technical"><Accordion.Heading><Accordion.Trigger>{t("technical")}<Accordion.Indicator/></Accordion.Trigger></Accordion.Heading><Accordion.Panel><Accordion.Body>
            <p className="source">{pattern ? t("patternSource") : path ?? t("noImage")}</p>
            {statusDetail !== status && <p>{statusDetail}</p>}
            {preview && g && <><p>{t("rasterHash")}<br/><code>{preview.sha256}</code></p><p>{t("inputHash")}<br/><code>{preview.input_sha256}</code></p><p>{t("geometry", { width: g.paper_width, height: g.height, bytes: 48 * g.height, x: g.paper_x })}</p><p>{t("clippedDots", { count: preview.clipped_dot_count })}</p><p className="source">{preview.settings_path ? t("settingsPath", { path: preview.settings_path }) : t("defaultSettings")}</p></>}
            <Checkbox variant="secondary" isSelected={headView} onChange={setHeadView}><Checkbox.Content><Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>{t("showHead")}</Checkbox.Content></Checkbox>
            <p>{t("headHint")}</p>
            <p>{t("formatsHint")}</p>
            <p>{t("paperHint")}</p>
            <p>{t("rotationDetail")}</p>
            <p>{t("printerHint")}</p>
            <p>{t("printHint")}</p>
          </Accordion.Body></Accordion.Panel></Accordion.Item></Accordion>
          </div>
          <section className="print-action" aria-label={t("printAction")}>
            <div className="print-row">
            <div className="field"><Label htmlFor="copies">{t("copies")}</Label><Input id="copies" aria-label={t("printCopies")} variant="secondary" type="number" min={1} max={10} step={1} value={pattern ? 1 : Number.isFinite(copies) ? copies : ""} disabled={disabled || pattern} aria-invalid={!pattern && (!Number.isInteger(copies) || copies < 1 || copies > 10)} aria-describedby="print-reason" onChange={(e) => changeCopies(e.target.valueAsNumber)}/></div>
            <Button variant={hasLabel ? "primary" : "secondary"} className="print-button" isDisabled={!ready} onPress={print}>{t((pattern || copies === 1) ? "printOne" : "printMany", { count: pattern ? 1 : Number.isFinite(copies) ? copies : "—" })}</Button>
            </div>
            <div role="status" aria-live="polite"><p id="print-reason" className={numericError || previewError ? "warning" : "help"}>{emptyHint ? "" : status}</p>{job && !job.finished && <ProgressBar aria-label={t("transferProgress")} value={job.sent_bytes} maxValue={job.total_bytes || 1} color="default"><ProgressBar.Track><ProgressBar.Fill/></ProgressBar.Track></ProgressBar>}</div>
            {job && !job.finished && <Button variant="secondary" isDisabled={!canCancel} onPress={cancel}>{t("cancel")}</Button>}
          </section>
        </aside>
      </div>
    </main></I18nProvider>
  );
}
