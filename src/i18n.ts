import en from "./locales/en.json";
import ko from "./locales/ko.json";
import type { AppError, LocalizedMessage } from "./contracts";

export type Locale = "en" | "ko";
export type MessageKey = keyof typeof en;
const catalogs: Record<Locale, Record<MessageKey, string>> = { en, ko };
export type Notice = { key: MessageKey; params?: Record<string, string | number | Notice>; error?: unknown };
export const msg = (key: MessageKey, params?: Notice["params"]): Notice => ({ key, params });
export const errorNotice = (error: unknown): Notice => ({ key: "error", error });
export const systemLocale = (language: string): Locale => /^ko(?:-|$)/i.test(language) ? "ko" : "en";

export function translate(locale: Locale, key: MessageKey, params: Record<string, string | number> = {}): string {
  return catalogs[locale][key].replace(/\{(\w+)\}/g, (token, name: string) => String(params[name] ?? token));
}
function errorMessage(locale: Locale, message: LocalizedMessage): string | null {
  if (!Object.hasOwn(en, message.key)) return null;
  const params: Record<string, string> = { ...message.params };
  if (message.cause) {
    const cause = errorMessage(locale, message.cause);
    if (cause === null) return null;
    params.cause = cause;
  }
  return translate(locale, message.key as MessageKey, params);
}
export function translateError(locale: Locale, error: unknown): string {
  if (typeof error === "object" && error !== null && "detail" in error) {
    const value = error as AppError;
    return (value.message && errorMessage(locale, value.message)) ?? String(value.detail);
  }
  return String(error);
}
export function translateNotice(locale: Locale, notice: Notice | null): string {
  if (!notice) return "";
  const params = Object.fromEntries(Object.entries(notice.params ?? {}).map(([name, value]) => [name, typeof value === "object" ? translateNotice(locale, value) : value]));
  if (notice.key === "error") params.detail = translateError(locale, notice.error);
  return translate(locale, notice.key, params);
}
