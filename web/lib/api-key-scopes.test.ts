import { describe, expect, it } from "vitest";

import { DEFAULT_SCOPES, withScope } from "./api-key-scopes";
import { ApiKeyScope } from "./types/enums";

describe("a key's scopes", () => {
  it("start sending and reading", () => {
    expect(DEFAULT_SCOPES).toEqual([ApiKeyScope.Ingest, ApiKeyScope.Read]);
  });

  it("keep the console's order whatever order they were ticked in", () => {
    expect(withScope([ApiKeyScope.Read], ApiKeyScope.Ingest, true)).toEqual([
      ApiKeyScope.Ingest,
      ApiKeyScope.Read,
    ]);
    expect(withScope([ApiKeyScope.Ingest, ApiKeyScope.Read], ApiKeyScope.Ingest, false)).toEqual([
      ApiKeyScope.Read,
    ]);
  });
});
