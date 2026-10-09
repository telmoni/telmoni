import { describe, expect, it } from "vitest";

import {
  dayField,
  eventsLabel,
  exportWindow,
  failureText,
  rangeLabel,
} from "./audit-exports";

// Local construction throughout, as the dialog's own days are: a custom
// range is the days the person picked, in their own time zone.
const NOW = new Date(2026, 9, 8, 15, 30);

describe("an export's window", () => {
  it("is the whole chain, up to now, for all time", () => {
    expect(exportWindow("all", "", "", NOW)).toEqual({ from: null, to: null });
  });

  it("runs back from now for the last days", () => {
    expect(exportWindow("7d", "", "", NOW)).toEqual({
      from: new Date(NOW.getTime() - 7 * 24 * 60 * 60 * 1000).toISOString(),
      to: null,
    });
  });

  it("runs from the first day's start to the last day's end for a custom range", () => {
    expect(exportWindow("custom", "2026-09-01", "2026-09-30", NOW)).toEqual({
      from: new Date(2026, 8, 1).toISOString(),
      to: new Date(2026, 9, 1).toISOString(),
    });
  });

  it("ends at now when the custom range runs through today", () => {
    expect(exportWindow("custom", "2026-10-01", "2026-10-08", NOW)).toEqual({
      from: new Date(2026, 9, 1).toISOString(),
      to: null,
    });
  });

  it("is refused while the custom range is not whole", () => {
    expect(exportWindow("custom", "", "2026-10-08", NOW)).toBeNull();
    expect(exportWindow("custom", "2026-10-05", "2026-10-01", NOW)).toBeNull();
    expect(exportWindow("custom", "2026-10-09", "2026-10-10", NOW)).toBeNull();
    expect(exportWindow("custom", "not a day", "2026-10-08", NOW)).toBeNull();
  });
});

describe("an export's labels", () => {
  it("spells today as a date field does", () => {
    expect(dayField(NOW)).toBe("2026-10-08");
  });

  it("names a custom range by its last day, not the midnight after it", () => {
    const label = rangeLabel(
      {
        range_from: new Date(2026, 8, 1).toISOString(),
        range_to: new Date(2026, 9, 1).toISOString(),
      },
      NOW,
    );
    expect(label).toContain("Sep");
    expect(label).toContain("30");
    expect(label).not.toContain("Oct");
  });

  it("says everything up to its end for the whole chain", () => {
    expect(rangeLabel({ range_from: null, range_to: NOW.toISOString() }, NOW)).toMatch(
      /^Everything to /,
    );
  });

  it("counts one event, and many", () => {
    expect(eventsLabel(1)).toBe("1 event");
    expect(eventsLabel(0)).toBe("0 events");
  });

  it("says a range too large is the range's fault", () => {
    expect(failureText({ failure: "too_large" })).toMatch(/shorter/);
    expect(failureText({ failure: "error" })).toMatch(/Try again/);
  });
});
