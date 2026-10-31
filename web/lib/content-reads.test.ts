import { describe, expect, it } from "vitest";

import { contentReaders } from "./content-reads";
import { ResourceKind } from "./types/enums";

function read(actorId: string, createdAt: string, traceIds: string[]) {
  return {
    id: `${actorId}-${createdAt}`,
    actorId,
    action: "viewed",
    resourceKind: ResourceKind.Project,
    resourceId: "project_1",
    createdAt,
    metadata: { trace_ids: traceIds, fields: traceIds.length },
  };
}

describe("contentReaders", () => {
  it("counts each reader's reads and distinct traces, the latest reader first", () => {
    expect(
      contentReaders([
        read("user_2", "2026-10-30T09:00:00Z", ["t3"]),
        read("user_1", "2026-10-29T10:00:00.5Z", ["t1", "t2"]),
        read("user_1", "2026-10-29T10:00:00Z", ["t2"]),
      ]),
    ).toEqual([
      { actorId: "user_2", reads: 1, traces: 1, last: "2026-10-30T09:00:00Z" },
      { actorId: "user_1", reads: 2, traces: 2, last: "2026-10-29T10:00:00.5Z" },
    ]);
  });

  it("counts a read whose details it cannot read, naming no trace", () => {
    expect(contentReaders([{ ...read("user_1", "2026-10-29T10:00:00Z", []), metadata: null }]))
      .toEqual([{ actorId: "user_1", reads: 1, traces: 0, last: "2026-10-29T10:00:00Z" }]);
  });
});
