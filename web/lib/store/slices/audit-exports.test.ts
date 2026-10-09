import { describe, expect, it } from "vitest";

import type { AuditExport } from "@/lib/types/audit-export";

import { createStore } from "../index";

function store() {
  return createStore({ user: null, flags: { members: true }, roles: {}, projects: [] });
}

function entry(id: string, overrides: Partial<AuditExport> = {}): AuditExport {
  return {
    id,
    format: "json",
    range_from: null,
    range_to: "2026-10-08T12:00:00Z",
    status: "queued",
    failure: null,
    row_count: null,
    bytes: null,
    created_at: "2026-10-08T12:00:00Z",
    finished_at: null,
    expires_at: null,
    downloaded_at: null,
    ...overrides,
  };
}

describe("the audit exports slice", () => {
  it("holds nothing until the bell has asked", () => {
    expect(store().getState().auditExports).toEqual({});
  });

  it("lays a read's list over its own, and answers what it held", () => {
    const s = store();
    expect(s.getState().replaceAuditExports("org_a", [entry("one")], 0)).toEqual([]);
    expect(
      s.getState().replaceAuditExports("org_a", [entry("one", { status: "ready" })], 0),
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
        [entry("just started", { status: "running" })],
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
      s.getState().replaceAuditExports("org_a", [entry("theirs", { status: "ready" })], 1),
    ).toEqual([entry("theirs")]);
  });

  it("notes a download once, and leaves the rest as they were", () => {
    const s = store();
    s.getState().replaceAuditExports(
      "org_a",
      [entry("taken", { status: "ready" }), entry("left", { status: "ready" })],
      0,
    );
    s.getState().markAuditExportDownloaded("taken");
    const [taken, left] = s.getState().auditExports.org_a!;
    expect(taken!.downloaded_at).not.toBeNull();
    expect(left!.downloaded_at).toBeNull();
  });

  // A download that finds the file gone takes it off the bell at once,
  // rather than leaving it offered until the next read.
  it("forgets one no longer kept, and only that one", () => {
    const s = store();
    s.getState().replaceAuditExports(
      "org_a",
      [entry("gone", { status: "ready" }), entry("kept", { status: "ready" })],
      0,
    );
    s.getState().forgetAuditExport("gone");
    expect(s.getState().auditExports.org_a?.map((e) => e.id)).toEqual(["kept"]);
  });
});
