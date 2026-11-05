import { describe, expect, it } from "vitest";

import { modeChanges, readerName, readersByProject, scrubbedSpans } from "./data-sheet";
import type { AuditEvent } from "@/lib/server/data";

const event = (over: Partial<AuditEvent>): AuditEvent => ({
  id: "e",
  actorId: "user_1",
  action: "updated",
  resourceKind: "project",
  resourceId: "project_1",
  createdAt: "2026-11-01T00:00:00Z",
  metadata: null,
  inProject: "project_1",
  ...over,
});

describe("modeChanges", () => {
  it("keeps the content mode's changes, oldest first, and passes over the rest", () => {
    const changes = modeChanges([
      event({ createdAt: "2026-11-03T00:00:00Z", metadata: { content_mode: { from: "on", to: "off" }, content_deleted: true } }),
      event({ createdAt: "2026-11-02T00:00:00Z", metadata: { retention_days: { from: 30, to: 7 } } }),
      event({ createdAt: "2026-11-01T00:00:00Z", actorId: "user_2", metadata: { content_mode: { from: "off", to: "on" } } }),
    ]);
    expect(changes).toEqual([
      { at: "2026-11-01T00:00:00Z", from: "off", to: "on", by: "user_2", deleted: false },
      { at: "2026-11-03T00:00:00Z", from: "on", to: "off", by: "user_1", deleted: true },
    ]);
  });
});

describe("readersByProject", () => {
  it("counts each reader's reads of each project's content, the latest first", () => {
    const readers = readersByProject([
      event({ action: "viewed", actorId: "user_1", createdAt: "2026-11-01T00:00:00Z" }),
      event({ action: "viewed", actorId: "user_1", createdAt: "2026-11-04T00:00:00Z" }),
      event({ action: "viewed", actorId: "key:k1", createdAt: "2026-11-03T00:00:00Z" }),
      event({ action: "viewed", inProject: "project_2", createdAt: "2026-11-02T00:00:00Z" }),
      event({ action: "viewed", resourceKind: "organization", inProject: null }),
      event({ action: "updated" }),
    ]);
    expect(readers.get("project_1")).toEqual([
      { who: "user_1", reads: 2, last: "2026-11-04T00:00:00Z" },
      { who: "key:k1", reads: 1, last: "2026-11-03T00:00:00Z" },
    ]);
    expect(readers.get("project_2")).toEqual([{ who: "user_1", reads: 1, last: "2026-11-02T00:00:00Z" }]);
  });

  it("names a key by its id and a person by name", () => {
    expect(readerName("key:k1", () => "Ada")).toBe("API key k1");
    expect(readerName("user_1", () => "Ada")).toBe("Ada");
  });
});

describe("scrubbedSpans", () => {
  it("reads how many spans arrived carrying the scrubbers' counts", () => {
    const posture = {
      days: 30,
      since: "2026-10-06T00:00:00Z",
      spans: 100,
      sent: [
        { attribute: "telmoni.scrubbed", spans: 12 },
        { attribute: "gen_ai.request.model", spans: 80 },
      ],
      retention: { span_days: 30, max_span_days: 360, rollup_months: 13 },
    };
    expect(scrubbedSpans(posture)).toBe(12);
    expect(scrubbedSpans({ ...posture, sent: [] })).toBe(0);
  });
});
