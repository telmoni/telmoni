import { describe, expect, it } from "vitest";

import { DEFAULT_COLUMNS } from "@/lib/traces";

import { DEFAULT_PREFS, orderedColumns, readPrefs, refreshLabel } from "./trace-prefs";

describe("what a browser kept of the trace list's look", () => {
  it("reads every choice, the columns in the list's order", () => {
    expect(
      readPrefs(
        JSON.stringify({
          columns: ["duration", "started", "models"],
          density: "compact",
          refresh: 60,
          filters: false,
          peekFull: true,
          peekWidth: 720,
        }),
      ),
    ).toEqual({
      columns: ["started", "duration", "models"],
      density: "compact",
      refresh: 60,
      filters: false,
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
          columns: ["spend", 7, "agent"],
          density: "airy",
          refresh: 45,
          filters: "yes",
          peekFull: 1,
          peekWidth: 12.5,
        }),
      ),
    ).toEqual({ ...DEFAULT_PREFS, columns: ["agent"] });
    // A width no panel stands at, stored by hand or by an older build.
    expect(readPrefs(JSON.stringify({ peekWidth: 40 })).peekWidth).toBeNull();
    expect(readPrefs(JSON.stringify({ peekWidth: "720" })).peekWidth).toBeNull();
    expect(readPrefs(JSON.stringify({ density: "compact" }))).toEqual({
      ...DEFAULT_PREFS,
      columns: DEFAULT_COLUMNS,
      density: "compact",
    });
  });

  it("orders columns as the list does, each once", () => {
    expect(orderedColumns(["tools", "agent", "agent", "run_id"])).toEqual(["agent", "tools"]);
    expect(orderedColumns([])).toEqual([]);
  });

  it("says how often the page reads itself again", () => {
    expect(refreshLabel(0)).toBe("Off");
    expect(refreshLabel(30)).toBe("30 s");
    expect(refreshLabel(60)).toBe("1 min");
    expect(refreshLabel(900)).toBe("15 min");
  });
});
