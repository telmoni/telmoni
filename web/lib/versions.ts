import { z } from "zod";

import { formatScoreValue } from "@/lib/scores";
import { formatCost, formatDuration } from "@/lib/traces";
import { ChangeKind } from "@/lib/types/enums";

// One agent's versions side by side, as the Agents page's panel reads them:
// the answer's shape, parsed on the server by its entity and in the browser
// by the panel, and each row's change from the version before it.

/** What the rows are split by: the words a change is kinded by. */
export const VERSIONS_BY = [ChangeKind.Release, ChangeKind.Model, ChangeKind.Prompt] as const;
export type VersionsBy = (typeof VERSIONS_BY)[number];

const ChoiceSchema = z.object({ value: z.string(), traces: z.number().int() });

const VersionSchema = z.object({
  /** Empty for the traces that carried none. */
  value: z.string(),
  first_at: z.string(),
  traces: z.number().int(),
  failed_traces: z.number().int(),
  /** None before one of its traces has ended. */
  p50_ms: z.number().nullable(),
  p95_ms: z.number().nullable(),
  median_spend: z.number().nullable(),
  scores: z.array(z.object({ name: z.string(), mean: z.number(), count: z.number().int() })),
});
export type Version = z.infer<typeof VersionSchema>;

export const VersionsSchema = z.object({
  days: z.number().int(),
  by: z.enum(VERSIONS_BY),
  /** The environment the rows are of, empty for the one with no name. */
  environment: z.string(),
  environments: z.array(ChoiceSchema),
  /** The prompt the rows are versions of, where they are a prompt's. */
  prompt: z.string().nullable(),
  prompts: z.array(ChoiceSchema),
  /** The newest first seen first, the traces that carried none last. */
  versions: z.array(VersionSchema),
  /** The scores the rows show. */
  scores: z.array(z.string()),
  more: z.boolean(),
});
export type Versions = z.infer<typeof VersionsSchema>;

/** What a read of an agent's versions asks: an environment or a prompt left
 *  out is the server's to choose, and an empty environment the one with no name. */
export type VersionsAsk = { agent: string; by: VersionsBy; environment?: string; prompt?: string };

/** `ask` as a query: the agent a parameter, never a path, since a name is any
 *  text, `..` among it. */
export function versionsQuery(ask: VersionsAsk): URLSearchParams {
  const params = new URLSearchParams({ agent: ask.agent, by: ask.by });
  if (ask.environment !== undefined) params.set("environment", ask.environment);
  if (ask.prompt !== undefined) params.set("prompt", ask.prompt);
  return params;
}

/** An agent's versions, through the console's own route. */
export function versionsReadHref(projectId: string, ask: VersionsAsk): string {
  const params = versionsQuery(ask);
  params.set("project", projectId);
  return `/api/v1/agent-versions?${params.toString()}`;
}

/** The share of a version's traces that failed, none where it has none. */
export function failedShare(version: Pick<Version, "traces" | "failed_traces">): number | null {
  return version.traces > 0 ? version.failed_traces / version.traces : null;
}

/** A row's change from the version before it, each in the row's own unit and
 *  none where either side has no value. */
export type VersionDelta = {
  /** In percentage points. */
  failed: number | null;
  p50_ms: number | null;
  p95_ms: number | null;
  /** In US dollars. */
  median_spend: number | null;
  /** By score name, in the score's own scale. */
  scores: Record<string, number | null>;
};

const less = (now: number | null | undefined, before: number | null | undefined): number | null =>
  now == null || before == null ? null : now - before;

/** Each row's change from the next row down, the version first seen before
 *  it; none for the oldest, nor for the traces that carried no version, which
 *  no row is measured against either. */
export function deltas(rows: readonly Version[]): (VersionDelta | null)[] {
  return rows.map((row, at) => {
    const before = rows[at + 1];
    if (row.value === "" || !before || before.value === "") return null;
    // A share of traces none of which has ended is no evidence of a fall.
    const ended = row.p50_ms !== null && before.p50_ms !== null;
    const share = ended ? less(failedShare(row), failedShare(before)) : null;
    const mean = (of: Version, name: string) => of.scores.find((score) => score.name === name)?.mean;
    return {
      failed: share === null ? null : share * 100,
      p50_ms: less(row.p50_ms, before.p50_ms),
      p95_ms: less(row.p95_ms, before.p95_ms),
      median_spend: less(row.median_spend, before.median_spend),
      scores: Object.fromEntries(
        row.scores.map((score) => [score.name, less(score.mean, mean(before, score.name))]),
      ),
    };
  });
}

const PERCENT = new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 1 });
const POINTS = new Intl.NumberFormat("en-US", { maximumFractionDigits: 1 });

/** A share of traces: `12.5%`. */
export function formatShare(share: number): string {
  return PERCENT.format(share);
}

/** A change's size as it is written, by what it is a change of: a failed
 *  share's in points, the rest as their own values read. */
const DELTA_SIZE = {
  failed: (size: number) => `${POINTS.format(size)} pts`,
  duration: formatDuration,
  cost: formatCost,
  score: formatScoreValue,
} as const;
export type DeltaKind = keyof typeof DELTA_SIZE;

/** A change as it reads, its sign before its size (`+340 ms`, `−$0.01`); none
 *  where the size as written is zero, which is no change to a reader. */
export function deltaWords(kind: DeltaKind, delta: number): string | null {
  const size = DELTA_SIZE[kind](Math.abs(delta));
  return /[1-9]/.test(size) ? `${delta > 0 ? "+" : "−"}${size}` : null;
}

/** Each split's name as its tab reads, as its column is headed, and as a
 *  sentence names one row's. */
export const BY_LABEL: Record<VersionsBy, { tab: string; column: string; one: string }> = {
  [ChangeKind.Release]: { tab: "Releases", column: "Release", one: "release" },
  [ChangeKind.Model]: { tab: "Models", column: "Model", one: "model" },
  [ChangeKind.Prompt]: { tab: "Prompts", column: "Version", one: "version" },
};

/** An environment as its picker names it. */
export function environmentLabel(environment: string): string {
  return environment === "" ? "(no environment)" : environment;
}
