import { describe, expect, it } from "vitest";
import en from "./locales/en.json";
import ko from "./locales/ko.json";
import { msg, systemLocale, translate, translateError, translateNotice, type MessageKey } from "./i18n";

describe("message catalogs", () => {
  it("has exactly the same nonempty keys and placeholders in both languages", () => {
    expect(Object.keys(ko).sort()).toEqual(Object.keys(en).sort());
    const placeholders = (text: string) => [...new Set(text.match(/\{\w+\}/g) ?? [])].sort();
    for (const key of Object.keys(en) as MessageKey[]) {
      expect(en[key].trim(), key).not.toBe("");
      expect(ko[key].trim(), key).not.toBe("");
      expect(placeholders(ko[key]), key).toEqual(placeholders(en[key]));
    }
    expect(systemLocale("KO-kr")).toBe("ko");
    expect(systemLocale("kok-IN")).toBe("en");
    expect(systemLocale("")).toBe("en");
  });

  it("preserves external causes, old errors and unknown future metadata without sentence matching", () => {
    const cause = { key: "err.bluetooth", params: { e: "external {cause} /프린터" } };
    const error = { code: "bluetooth_error", detail: "English fallback", message: { key: "err.observerRestart", params: {}, cause } };
    expect(translateError("en", error)).toBe("Check Bluetooth settings, access permissions, and printer power: external {cause} /프린터 Restart the OpenLabel process to initialize the name observer adapter again.");
    expect(translateError("ko", error)).toBe("Bluetooth 설정·접근 권한과 프린터 전원을 확인하세요: external {cause} /프린터 이름 확인 어댑터를 다시 초기화하려면 Openlabel 프로세스를 다시 시작하세요.");
    for (const locale of ["en", "ko"] as const) {
      expect(translateError(locale, { code: "old", detail: "original cause" })).toBe("original cause");
      expect(translateError(locale, { ...error, message: { key: "future", params: {} } })).toBe("English fallback");
      expect(translate(locale, "settingsPath", { path: "/{path}/한글.svg" })).toContain("/{path}/한글.svg");
    }
    expect(translateNotice("en", msg("numericInvalid", { field: msg("density"), min: 1, max: 15, kind: msg("integer") }))).toBe("Numeric input: set Density to an integer between 1 and 15.");
  });
});
