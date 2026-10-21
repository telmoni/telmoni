// Every time the console shows, written in the reader's own zone and locale.
// The server renders each page and cannot ask the browser how it writes a
// time, so the browser leaves both in a cookie (`components/local-time.tsx`)
// and the server writes each time as the browser will: nothing changes once
// the page comes alive. Until the cookie is there, the server writes UTC.

/** The cookie the browser leaves its zone and locale in. */
export const LOCAL_TIME_COOKIE = "telmoni-local-time";

/** How a browser writes a time: the zone it reads it in and the locale it
 *  writes it in, each as `Intl` resolves it. */
export type Reader = { timeZone: string; locale: string };

/** `precise` is to the millisecond: a span's start, where two calls a few
 *  milliseconds apart must read apart. */
export type TimeMode = "date" | "datetime" | "precise";

/** The fields a precise time is written in: `toLocaleString`'s own, so it
 *  reads as every other time in the console does, and the milliseconds. */
const PRECISE: Intl.DateTimeFormatOptions = {
  year: "numeric",
  month: "numeric",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
  second: "2-digit",
  fractionalSecondDigits: 3,
};

/** `date` as `reader` writes it, or as this runtime does without one. */
export function formatTime(date: Date, mode: TimeMode, reader?: Reader): string {
  const locale = reader?.locale;
  const timeZone = reader?.timeZone;
  if (mode === "precise") return date.toLocaleString(locale, { ...PRECISE, timeZone });
  if (mode === "date") return date.toLocaleDateString(locale, { timeZone });
  return date.toLocaleString(locale, { timeZone });
}

/** `date` in UTC, as the server writes it before the browser has said how
 *  it writes one. */
export function formatUtc(date: Date, mode: TimeMode): string {
  const iso = date.toISOString();
  if (mode === "date") return iso.slice(0, 10);
  return `${iso.slice(0, 10)} ${iso.slice(11, mode === "precise" ? 23 : 16)} UTC`;
}

/** The cookie's value for `reader`, encoded as a cookie's value must be. */
export function readerCookie(reader: Reader): string {
  return encodeURIComponent(`${reader.timeZone}|${reader.locale}`);
}

const ZONE = /^[A-Za-z0-9_+\-/:]{1,64}$/;
const LOCALE = /^[A-Za-z0-9-]{2,64}$/;

/** The reader the cookie names, as the server reads it decoded, or none
 *  where it names a zone or a locale this runtime cannot write a time in. */
export function parseReader(value: string | undefined): Reader | null {
  const [timeZone, locale, ...rest] = value?.split("|") ?? [];
  if (rest.length > 0 || !timeZone || !locale) return null;
  if (!ZONE.test(timeZone) || !LOCALE.test(locale)) return null;
  try {
    const [supported] = Intl.DateTimeFormat.supportedLocalesOf(locale);
    if (!supported) return null;
    // Throws on a zone this runtime does not know.
    Intl.DateTimeFormat(supported, { timeZone });
    return { timeZone, locale: supported };
  } catch {
    return null;
  }
}

/** This browser's zone and locale. */
export function browserReader(): Reader {
  const { timeZone, locale } = new Intl.DateTimeFormat().resolvedOptions();
  return { timeZone, locale };
}
