import { describe, expect, it } from "vitest";

import {
  type TimeMode,
  browserReader,
  formatTime,
  formatUtc,
  parseReader,
  readerCookie,
} from "./local-time";

const MODES: TimeMode[] = ["date", "datetime", "precise"];

describe("the reader's zone and locale, from the cookie the browser leaves", () => {
  it("reads a zone and a locale this runtime can write a time in", () => {
    expect(parseReader("America/Chicago|en-US")).toEqual({
      timeZone: "America/Chicago",
      locale: "en-US",
    });
    expect(parseReader("America/Argentina/Buenos_Aires|es-AR")).toEqual({
      timeZone: "America/Argentina/Buenos_Aires",
      locale: "es-AR",
    });
    expect(parseReader("Etc/GMT+5|en-GB")).toEqual({ timeZone: "Etc/GMT+5", locale: "en-GB" });
    expect(parseReader("UTC|zh-Hant-TW")).toEqual({ timeZone: "UTC", locale: "zh-Hant-TW" });
    expect(parseReader("Europe/Berlin|DE-de")).toEqual({ timeZone: "Europe/Berlin", locale: "de-DE" });
  });

  it("reads none where the cookie is missing or names no zone and locale", () => {
    expect(parseReader(undefined)).toBeNull();
    expect(parseReader("")).toBeNull();
    expect(parseReader("America/Chicago")).toBeNull();
    expect(parseReader("America/Chicago|")).toBeNull();
    expect(parseReader("|en-US")).toBeNull();
    expect(parseReader("America/Chicago|en-US|en-GB")).toBeNull();
    expect(parseReader("Mars/Olympus_Mons|en-US")).toBeNull();
    expect(parseReader("America/Chicago|en_US")).toBeNull();
    expect(parseReader("America/Chicago|en-US; path=/")).toBeNull();
    expect(parseReader("America/Chicago|e")).toBeNull();
    expect(parseReader(`${"A".repeat(65)}|en-US`)).toBeNull();
  });

  it("reads back whatever this runtime is, through the cookie", () => {
    const reader = browserReader();
    expect(parseReader(decodeURIComponent(readerCookie(reader)))).toEqual(reader);
    expect(readerCookie({ timeZone: "America/Port-au-Prince", locale: "fr-HT" })).toMatch(
      /^[A-Za-z0-9%_.!~*'()-]+$/,
    );
  });
});

describe("a time, as the reader writes it", () => {
  const instant = new Date("2026-10-22T02:00:00.123Z");

  it("is what the browser writes unasked, given the browser's own zone and locale", () => {
    const reader = browserReader();
    for (const mode of MODES) {
      expect(formatTime(instant, mode, reader)).toBe(formatTime(instant, mode));
    }
    expect(formatTime(instant, "date")).toBe(instant.toLocaleDateString());
    expect(formatTime(instant, "datetime")).toBe(instant.toLocaleString());
  });

  it("falls on the reader's own day, in the reader's own order", () => {
    expect(formatTime(instant, "date", { timeZone: "America/Chicago", locale: "en-US" })).toBe(
      "10/21/2026",
    );
    expect(formatTime(instant, "date", { timeZone: "UTC", locale: "en-US" })).toBe("10/22/2026");
    expect(formatTime(instant, "date", { timeZone: "UTC", locale: "en-GB" })).toBe("22/10/2026");
  });

  it("is UTC before the browser has said how it writes one", () => {
    expect(formatUtc(instant, "date")).toBe("2026-10-22");
    expect(formatUtc(instant, "datetime")).toBe("2026-10-22 02:00 UTC");
    expect(formatUtc(instant, "precise")).toBe("2026-10-22 02:00:00.123 UTC");
  });
});
