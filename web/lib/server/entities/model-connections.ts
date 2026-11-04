import { cache } from "react";
import { z } from "zod";

import { ModelProvider } from "@/lib/types/enums";

import { valueOf } from "./read";
import { readLane } from "./traces";

// A project's connection to a model on the customer's own key, as the lane
// answers it: what it reaches and its key's last four characters, never the key.
export const ModelConnectionSchema = z.object({
  id: z.string(),
  name: z.string(),
  provider: z.enum(ModelProvider),
  base_url: z.string().nullable(),
  region: z.string().nullable(),
  models: z.array(z.string()),
  is_default: z.boolean(),
  secret_hint: z.string(),
  signs_in: z.boolean(),
  refused_at: z.string().nullable(),
  refused_reason: z.string().nullable(),
  last_used_at: z.string().nullable(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type ModelConnection = z.infer<typeof ModelConnectionSchema>;

// What a customer's role or pool trusts for a keyless connection; `null` where none is signed.
export const FederationSetupSchema = z.object({ issuer: z.string(), subject: z.string(), aws_audience: z.string() });
export type FederationSetup = z.infer<typeof FederationSetupSchema>;

export const ModelConnectionsSchema = z.object({
  connections: z.array(ModelConnectionSchema),
  federation: FederationSetupSchema.nullable(),
});
export type ModelConnectionsAnswer = z.infer<typeof ModelConnectionsSchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads its connections.
export const fetchModelConnectionsAnswer = cache(
  async (projectId: string): Promise<ModelConnectionsAnswer | null> =>
    valueOf(await readLane("fetchModelConnections", projectId, "/model-connections", ModelConnectionsSchema)),
);

export const fetchModelConnections = cache(
  async (projectId: string): Promise<ModelConnection[] | null> =>
    (await fetchModelConnectionsAnswer(projectId))?.connections ?? null,
);
