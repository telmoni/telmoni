import { describe, expect, it } from "vitest";

import { DEFAULT_COLUMNS } from "@/lib/traces";

import { DEFAULT_LAYOUT } from "./layout";
import { DEFAULT_PREFS, readPrefs, refreshLabel } from "./trace-prefs";

/** The default customized to steps and calls, as a kept layout might be. */
const COMPLEX = { ...DEFAULT_LAYOUT, depth: "calls" as const, open: "failed" as const, timeline: true };

describe("what a browser kept of the trace list's look", () => {
  it("reads every choice, the columns in the list's order", () => {
    expect(
      readPrefs(
        JSON.stringify({
          columns: ["duration", "started", "models"],
          density: "compact",
          refresh: 60,
          filters: false,
          filtersWidth: 240,
          peekFull: true,
          peekWidth: 720,
          layout: { view: null, options: COMPLEX },
        }),
      ),
    ).toEqual({
      columns: ["started", "duration", "models"],
      density: "compact",
      refresh: 60,
      filters: false,
      filtersWidth: 240,
      peekFull: true,
      peekWidth: 720,
      layout: { view: null, options: COMPLEX },
    });
  });

  it("falls back to the default for what it does not know, rather than applying it blind", () => {
    expect(readPrefs(null)).toEqual(DEFAULT_PREFS);
    expect(readPrefs("not json")).toEqual(DEFAULT_PREFS);
    expect(readPrefs("[]")).toEqual(DEFAULT_PREFS);
    expect(
      readPrefs(
        JSON.stringify({
          columns: ["spend", 7, "agent"],
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
    expect(readPrefs(JSON.stringify({ filtersWidth: 600 })).filtersWidth).toBeNull();
    expect(readPrefs(JSON.stringify({ filtersWidth: 244 })).filtersWidth).toBe(244);
    expect(readPrefs(JSON.stringify({ density: "compact" }))).toEqual({
      ...DEFAULT_PREFS,
      columns: DEFAULT_COLUMNS,
      density: "compact",
    });
  });

  it("says how often the page reads itself again", () => {
    expect(refreshLabel(0)).toBe("Off");
    expect(refreshLabel(30)).toBe("30 s");
    expect(refreshLabel(60)).toBe("1 min");
    expect(refreshLabel(900)).toBe("15 min");
  });
});
