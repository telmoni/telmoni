import { cache } from "react";
import { z } from "zod";

import { queryString } from "@/lib/list-query";
import { ChangeKind } from "@/lib/types/enums";

import type { Read } from "./read";
import { readLane } from "./traces";

// The first trace of an agent, in one environment, to carry a value its
// traces there had not, and when that trace started.
const changeFields = {
  agent: z.string(),
  /** Empty where its spans named none. */
  environment: z.string(),
  value: z.string(),
  changed_at: z.string(),
  trace_id: z.string(),
  /** Its agent's later changes of its kind that UTC day were not marked. */
  capped: z.boolean(),
};

/** A release or a model an agent changed to, or a prompt's version, which
 *  alone names its prompt, `value` the version. */
export const ChangeSchema = z.discriminatedUnion("kind", [
  z.object({ ...changeFields, kind: z.enum([ChangeKind.Release, ChangeKind.Model]) }),
  z.object({ ...changeFields, kind: z.literal(ChangeKind.Prompt), prompt: z.string() }),
]);
export type Change = z.infer<typeof ChangeSchema>;

/** The window's changes, the newest first, and whether it held more than the
 *  lane answers. */
export const ChangeListSchema = z.object({
  days: z.number().int(),
  more: z.boolean(),
  changes: z.array(ChangeSchema),
});
type ChangeList = z.infer<typeof ChangeListSchema>;

/** The project's changes over `days`, of `agents` and in `environments` where
 *  any are named, as the trace list's charts mark them. */
export const fetchChanges = cache(
  (
    projectId: string,
    days: number | undefined,
    agents: readonly string[],
    environments: readonly string[],
  ): Promise<Read<ChangeList>> =>
    readLane(
      "fetchChanges",
      projectId,
      `/changes${queryString({ days, agent: agents, environment: environments })}`,
      ChangeListSchema,
    ),
);
