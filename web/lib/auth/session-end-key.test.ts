import { describe, expect, it } from "vitest";

import { sessionEndKey } from "./session-end-key";

describe("sessionEndKey", () => {
  it("is auth's session row when the sign-in probe recorded one", () => {
    expect(sessionEndKey({ sessionRowId: "sess_row", sessionId: "ses_1" })).toBe("sess_row");
  });

  // ⚠ The case the fallback exists for: `callback/route.ts` seals
  // `sessionRowId: null` when its probe of `/me` fails, so the fallback
  // ensures the session can still be ended cleanly.
  it("falls back to the session id the exchange answered with", () => {
    expect(sessionEndKey({ sessionRowId: null, sessionId: "ses_1" })).toBe("ses_1");
  });

  it("is null when neither names the device", () => {
    expect(sessionEndKey({ sessionRowId: null, sessionId: null })).toBeNull();
  });
});
