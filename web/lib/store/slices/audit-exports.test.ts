import { describe, expect, it } from "vitest";

import type { AuditExport } from "@/lib/types/audit-export";

import { createStore } from "../index";

function store() {
  return createStore({ user: null, flags: { members: true }, projects: [] });
}

// One export in the state asked for, carrying what that state carries.
function entry(id: string, status: AuditExport["status"] = "queued"): AuditExport {
  const asked = {
    id,
    format: "json" as const,
    range_from: null,
    range_to: "2026-10-08T12:00:00Z",
    created_at: "2026-10-08T12:00:00Z",
  };
  const finished = { finished_at: "2026-10-08T12:05:00Z", expires_at: "2026-10-15T12:05:00Z" };
  switch (status) {
    case "ready":
      return { ...asked, status, row_count: 3, bytes: 1024, ...finished, downloaded_at: null };
    case "failed":
      return { ...asked, status, failure: "error", ...finished };
    default:
      return { ...asked, status };
  }
}

describe("the audit exports slice", () => {
  it("holds nothing until the bell has asked", () => {
    expect(store().getState().auditExports).toEqual({});
  });

  it("lays a read's list over its own, and answers what it held", () => {
    const s = store();
    expect(s.getState().replaceAuditExports("org_a", [entry("one")], 0)).toEqual([]);
    expect(
      s.getState().replaceAuditExports("org_a", [entry("one", "ready")], 0),
    ).toEqual([entry("one")]);
    expect(s.getState().auditExports.org_a?.[0]?.status).toBe("ready");
  });

  it("turns away a list read before an export started here", () => {
    const s = store();
    const leftWith = s.getState().auditExportsStarted;
    s.getState().addAuditExport("org_a", entry("just started"));
    // The read left before the start, so its list lacks it.
    expect(s.getState().replaceAuditExports("org_a", [], leftWith)).toBeNull();
    expect(s.getState().auditExports.org_a?.map((e) => e.id)).toEqual(["just started"]);
    // A read that left after it is laid over as usual.
    expect(
      s.getState().replaceAuditExports(
        "org_a",
        [entry("just started", "running")],
        s.getState().auditExportsStarted,
      ),
    ).not.toBeNull();
  });

  it("puts one just started in front of the organization's list", () => {
    const s = store();
    s.getState().replaceAuditExports("org_a", [entry("older")], 0);
    s.getState().addAuditExport("org_a", entry("newer"));
    expect(s.getState().auditExports.org_a?.map((e) => e.id)).toEqual(["newer", "older"]);
  });

  // Kept apart, so one still building in an organization the person left is
  // announced when they come back to it.
  it("keeps each organization's list apart", () => {
    const s = store();
    s.getState().replaceAuditExports("org_a", [entry("theirs")], 0);
    s.getState().addAuditExport("org_b", entry("ours"));
    expect(s.getState().auditExports).toEqual({
      org_a: [entry("theirs")],
      org_b: [entry("ours")],
    });
    expect(
      s.getState().replaceAuditExports("org_a", [entry("theirs", "ready")], 1),
    ).toEqual([entry("theirs")]);
  });

  it("notes a download once, and leaves the rest as they were", () => {
    const s = store();
    s.getState().replaceAuditExports(
      "org_a",
      [entry("taken", "ready"), entry("left", "ready")],
      0,
    );
    s.getState().markAuditExportDownloaded("taken");
    const [taken, left] = s.getState().auditExports.org_a!;
    expect(taken).toMatchObject({ status: "ready", downloaded_at: expect.any(String) });
    expect(left).toMatchObject({ status: "ready", downloaded_at: null });
  });

  // A download that finds the file gone takes it off the bell at once,
  // rather than leaving it offered until the next read.
  it("forgets one no longer kept, and only that one", () => {
    const s = store();
    s.getState().replaceAuditExports(
      "org_a",
      [entry("gone", "ready"), entry("kept", "ready")],
      0,
    );
    s.getState().forgetAuditExport("gone");
    expect(s.getState().auditExports.org_a?.map((e) => e.id)).toEqual(["kept"]);
  });
});
