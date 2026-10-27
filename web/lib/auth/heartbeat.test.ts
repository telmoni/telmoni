import { describe, expect, it } from "vitest";

import { SESSION_ENDED, afterRefusedHeartbeat } from "./heartbeat";

const refusal = (title: string) => ({ type: "about:blank", title, status: 401 });

describe("afterRefusedHeartbeat", () => {
  // An account deletion, an email change or a revoked device: auth ended the
  // session everywhere, and the sign-out route lands it on the home page.
  it("signs a session the console ended out, to the home page", () => {
    expect(afterRefusedHeartbeat(refusal(SESSION_ENDED))).toBe("/auth/logout");
  });

  it("sends a session that ran out to sign in again", () => {
    expect(afterRefusedHeartbeat(refusal("unauthenticated"))).toBe("/auth/login");
    expect(afterRefusedHeartbeat(refusal("refresh_failed"))).toBe("/auth/login");
  });

  // A body that did not parse is not evidence the session was ended.
  it("treats an unreadable refusal as a session that ran out", () => {
    expect(afterRefusedHeartbeat(null)).toBe("/auth/login");
    expect(afterRefusedHeartbeat("session_ended")).toBe("/auth/login");
  });
});
