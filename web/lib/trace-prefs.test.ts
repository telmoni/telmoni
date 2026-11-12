import { describe, expect, it } from "vitest";

import { DEFAULT_COLUMNS } from "@/lib/traces";

import { DEFAULT_PREFS, readPrefs } from "./trace-prefs";

describe("what a browser kept of the lists' look", () => {
  // An older build's keys — a refresh, a layout — are passed over, and a
  // column a list does not have with them.
  it("reads every choice, each list's columns in its own order", () => {
    expect(
      readPrefs(
        JSON.stringify({
          columns: ["duration", "started", "models"],
          sessionColumns: ["user", "steps", "last_seen"],
          userColumns: ["tokens", "scores", "sessions"],
          density: "compact",
          refresh: 60,
          filters: false,
          moreFilters: true,
          filtersWidth: 240,
          peekFull: true,
          peekWidth: 720,
          layout: { view: null, options: { depth: "calls" } },
        }),
      ),
    ).toEqual({
      columns: ["started", "duration", "models"],
      sessionColumns: ["last_seen", "user"],
      userColumns: ["sessions", "tokens"],
      density: "compact",
      filters: false,
      moreFilters: true,
      peekFull: true,
      peekWidth: 720,
    });
  });

  it("falls back to the default for what it does not know, rather than applying it blind", () => {
    expect(readPrefs(null)).toEqual(DEFAULT_PREFS);
    expect(readPrefs("not json")).toEqual(DEFAULT_PREFS);
    expect(readPrefs("[]")).toEqual(DEFAULT_PREFS);
    expect(
      readPrefs(
        JSON.stringify({
          columns: ["price", 7, "agent"],
          sessionColumns: "user",
          userColumns: { sessions: true },
          density: "airy",
          refresh: 45,
          filters: "yes",
          filtersWidth: 40,
          peekFull: 1,
          peekWidth: 12.5,
          layout: { view: null, options: {} },
        }),
      ),
    ).toEqual({ ...DEFAULT_PREFS, columns: ["agent"] });
    // A width no panel stands at, stored by hand or by an older build.
    expect(readPrefs(JSON.stringify({ peekWidth: 40 })).peekWidth).toBeNull();
    expect(readPrefs(JSON.stringify({ peekWidth: "720" })).peekWidth).toBeNull();
    // The filters' width is the page's alone: an older build's is forgotten.
    expect(readPrefs(JSON.stringify({ filtersWidth: 244 }))).toEqual(DEFAULT_PREFS);
    expect(readPrefs(JSON.stringify({ density: "compact" }))).toEqual({
      ...DEFAULT_PREFS,
      columns: DEFAULT_COLUMNS,
      density: "compact",
    });
    // The refresh interval is gone: an older build's is forgotten.
    expect(readPrefs(JSON.stringify({ refresh: 60 }))).toEqual(DEFAULT_PREFS);
  });
});
