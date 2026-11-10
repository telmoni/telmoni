import { describe, expect, it } from "vitest";

import {
  VersionsSchema,
  deltaWords,
  deltas,
  environmentLabel,
  failedShare,
  formatShare,
  versionsQuery,
  versionsReadHref,
  type Version,
} from "./versions";

const version = (value: string, patch: Partial<Version> = {}): Version => ({
  value,
  first_at: "2026-11-05T09:00:00Z",
  traces: 10,
  failed_traces: 0,
  p50_ms: 1_000,
  p95_ms: 2_000,
  median_spend: 0.01,
  scores: [],
  ...patch,
});

describe("deltas", () => {
  it("measures each row against the row below, in the row's own unit", () => {
    const [newer, older] = deltas([
      version("1.4.2", {
        failed_traces: 4,
        p50_ms: 2_400,
        p95_ms: 9_100,
        median_spend: 0.042,
        scores: [{ name: "helpful", mean: 0.6, count: 5 }],
      }),
      version("1.4.1", {
        failed_traces: 2,
        p50_ms: 2_100,
        p95_ms: 6_000,
        median_spend: 0.031,
        scores: [{ name: "helpful", mean: 0.8, count: 5 }],
      }),
    ]);
    expect(newer?.failed).toBeCloseTo(20, 9);
    expect(newer?.p50_ms).toBe(300);
    expect(newer?.p95_ms).toBe(3_100);
    expect(newer?.median_spend).toBeCloseTo(0.011, 9);
    expect(newer?.scores.helpful).toBeCloseTo(-0.2, 9);
    expect(older).toBeNull();
  });

  it("has none for a version alone", () => {
    expect(deltas([version("1.0.0")])).toEqual([null]);
    expect(deltas([])).toEqual([]);
  });

  it("has none where either side has no value, and a zero is a value", () => {
    const [newer] = deltas([
      version("b", { p50_ms: null, p95_ms: 500, median_spend: 0, scores: [{ name: "kept", mean: 1, count: 1 }] }),
      version("a", { p50_ms: 900, p95_ms: null, median_spend: 0, failed_traces: 0, scores: [] }),
    ]);
    expect(newer).toEqual({
      failed: null,
      p50_ms: null,
      p95_ms: null,
      median_spend: 0,
      scores: { kept: null },
    });
  });

  it("measures against a zero before it by its difference, never a ratio", () => {
    const [newer] = deltas([
      version("b", { failed_traces: 5, median_spend: 0.2 }),
      version("a", { failed_traces: 0, median_spend: 0 }),
    ]);
    expect(newer?.failed).toBe(50);
    expect(newer?.median_spend).toBe(0.2);
  });

  it("measures no row against the traces that carried none, nor those traces against any", () => {
    expect(deltas([version("b"), version("a"), version("")])).toEqual([
      expect.objectContaining({ p50_ms: 0 }),
      null,
      null,
    ]);
    expect(deltas([version("")])).toEqual([null]);
  });

  it("draws no change in the failed share where either side has no ended trace", () => {
    const running = version("b", { traces: 1, failed_traces: 0, p50_ms: null, p95_ms: null, median_spend: null });
    const ended = version("a", { failed_traces: 4 });
    // Its share is still its own to show: none of its one trace has failed yet.
    expect(failedShare(running)).toBe(0);
    expect(deltas([running, ended])[0]?.failed).toBeNull();
    expect(deltas([ended, running, version("0")])[0]?.failed).toBeNull();
    expect(deltas([ended, version("0")])[0]?.failed).toBeCloseTo(40, 9);
  });

  it("has no share of no traces", () => {
    expect(failedShare({ traces: 0, failed_traces: 0 })).toBeNull();
    expect(deltas([version("b", { traces: 0 }), version("a")])[0]?.failed).toBeNull();
  });
});

describe("the words a change reads as", () => {
  it("signs the size, each in its unit", () => {
    expect(deltaWords("failed", 20)).toBe("+20 pts");
    expect(deltaWords("failed", -4.25)).toBe("−4.3 pts");
    expect(deltaWords("duration", 300)).toBe("+300 ms");
    expect(deltaWords("duration", -3_100)).toBe("−3.1 s");
    expect(deltaWords("cost", 0.011)).toBe("+$0.011");
    expect(deltaWords("cost", -2)).toBe("−$2.00");
    expect(deltaWords("score", -0.2)).toBe("−0.20");
    expect(formatShare(0.125)).toBe("12.5%");
  });

  it("has none for a change that is written as zero", () => {
    // 12.34% against 12.30%: four hundredths of a point, written to a tenth.
    expect(deltaWords("failed", 12.34 - 12.3)).toBeNull();
    expect(deltaWords("failed", -0.04)).toBeNull();
    expect(deltaWords("failed", 0.05)).toBe("+0.1 pts");
    // Two means the same but for the arithmetic's last digit.
    expect(deltaWords("score", 0.1 + 0.2 - 0.3)).toBeNull();
    expect(deltaWords("score", 0.004)).toBeNull();
    expect(deltaWords("score", 0.01)).toBe("+0.01");
    expect(deltaWords("duration", 0.4)).toBeNull();
    expect(deltaWords("cost", 0)).toBeNull();
    expect(deltaWords("score", 0)).toBeNull();
  });

  it("names the environment with no name", () => {
    expect(environmentLabel("")).toBe("(no environment)");
    expect(environmentLabel("production")).toBe("production");
  });
});

describe("the read's address", () => {
  it("carries the agent as a parameter, whatever its name", () => {
    for (const agent of ["..", ".", "a/b", "100%", " padded ", "agént-日本"]) {
      const href = versionsReadHref("project_1", { agent, by: "release" });
      const url = new URL(href, "http://localhost");
      expect(url.pathname).toBe("/api/v1/agent-versions");
      expect(url.searchParams.get("agent")).toBe(agent);
      expect(url.searchParams.get("project")).toBe("project_1");
    }
  });

  it("tells the environment with no name from none asked for", () => {
    expect(versionsQuery({ agent: "a", by: "model" }).has("environment")).toBe(false);
    expect(versionsQuery({ agent: "a", by: "model", environment: "" }).toString()).toBe(
      "agent=a&by=model&environment=",
    );
    expect(versionsQuery({ agent: "a", by: "prompt", prompt: "triage:blue" }).get("prompt")).toBe("triage:blue");
  });
});

describe("VersionsSchema", () => {
  const answer = {
    days: 14,
    by: "release",
    environment: "",
    environments: [{ value: "", traces: 1 }],
    prompt: null,
    prompts: [],
    versions: [{ ...version("1.0.0"), p50_ms: null, p95_ms: null, median_spend: null }],
    scores: [],
    more: false,
  };

  it("reads an answer, a version with no ended trace among it", () => {
    expect(VersionsSchema.safeParse(answer).success).toBe(true);
  });

  it("refuses a split it does not know, a count that is none and a row without its traces", () => {
    expect(VersionsSchema.safeParse({ ...answer, by: "tool" }).success).toBe(false);
    expect(VersionsSchema.safeParse({ ...answer, days: "14" }).success).toBe(false);
    expect(VersionsSchema.safeParse({ ...answer, versions: [{ value: "1.0.0" }] }).success).toBe(false);
    expect(VersionsSchema.safeParse({ ...answer, prompt: undefined }).success).toBe(false);
  });
});
