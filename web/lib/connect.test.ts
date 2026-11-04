import { describe, expect, it } from "vitest";

import {
  CONNECT_REFUSALS,
  LINK_REFUSALS,
  accountSlackPath,
  connectOutcome,
  connectorsPath,
  linkOutcome,
} from "./connect";

describe("connectOutcome", () => {
  it("reads a provider connected, and each refusal a route writes", () => {
    expect(connectOutcome({ connected: "slack" })).toEqual({ connected: "slack" });
    for (const error of CONNECT_REFUSALS) {
      expect(connectOutcome({ error })).toEqual({ error });
    }
    expect(connectOutcome({})).toBeNull();
  });

  // A URL is anybody's to write: what no route writes is the plain failure.
  it("reads a word no route writes as the connection not starting", () => {
    expect(connectOutcome({ error: "made-up" })).toEqual({ error: "authorize" });
  });

  // One notice: a success the page cannot name never paints a refusal green.
  it("reads a refusal beside a provider it does not know as the refusal", () => {
    expect(connectOutcome({ connected: "bogus", error: "denied" })).toEqual({ error: "denied" });
  });

  it("reads a repeated parameter's first value", () => {
    expect(connectOutcome({ error: ["denied", "vendor"] })).toEqual({ error: "denied" });
  });

  it("writes what it reads back", () => {
    const path = connectorsPath("acme", "support", { error: "forbidden" });
    const query = Object.fromEntries(new URL(path, "https://console.test").searchParams);
    expect(connectOutcome(query)).toEqual({ error: "forbidden" });
  });
});

describe("linkOutcome", () => {
  it("reads a link, each refusal by its word, and nothing as nothing", () => {
    expect(linkOutcome({ slack: "linked" })).toBe("linked");
    for (const refusal of LINK_REFUSALS) {
      expect(linkOutcome({ slack: refusal })).toBe(refusal);
    }
    expect(linkOutcome({})).toBeNull();
  });

  it("reads a word no route writes as the plain failure", () => {
    expect(linkOutcome({ slack: "made-up" })).toBe("callback");
  });

  it("writes what it reads back, at the account page's Slack section", () => {
    const path = accountSlackPath("elsewhere");
    expect(path).toBe("/settings/account?slack=elsewhere#slack");
    const query = Object.fromEntries(new URL(path, "https://console.test").searchParams);
    expect(linkOutcome(query)).toBe("elsewhere");
    expect(accountSlackPath()).toBe("/settings/account#slack");
  });
});
