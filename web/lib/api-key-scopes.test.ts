import { describe, expect, it } from "vitest";

import { DEFAULT_SCOPES, withScope } from "./api-key-scopes";
import { ApiKeyScope } from "./types/enums";

describe("a key's scopes", () => {
  it("start without the change of evaluators", () => {
    expect(DEFAULT_SCOPES).toEqual([ApiKeyScope.Ingest, ApiKeyScope.Read]);
  });

  it("read wherever they change evaluators", () => {
    expect(withScope([ApiKeyScope.Ingest], ApiKeyScope.Evaluate, true)).toEqual([
      ApiKeyScope.Ingest,
      ApiKeyScope.Read,
      ApiKeyScope.Evaluate,
    ]);
    expect(withScope([ApiKeyScope.Read, ApiKeyScope.Evaluate], ApiKeyScope.Read, false)).toEqual([]);
    expect(withScope([ApiKeyScope.Read, ApiKeyScope.Evaluate], ApiKeyScope.Evaluate, false)).toEqual([
      ApiKeyScope.Read,
    ]);
  });
});
