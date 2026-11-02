import { cache } from "react";
import { z } from "zod";

import { valueOf } from "./read";
import { readLane } from "./traces";

// A project's own price for a provider's model, by every name it answers
// with: US dollars per million tokens, as the lane answers it.
export const ModelPriceSchema = z.object({
  provider: z.string(),
  models: z.array(z.string()),
  effective_from: z.string().nullable(),
  input: z.number(),
  output: z.number(),
  cache_read: z.number(),
  cache_write: z.number(),
});
export type ModelPrice = z.infer<typeof ModelPriceSchema>;

export const ModelPricesSchema = z.object({ prices: z.array(ModelPriceSchema) });

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads its prices.
export const fetchModelPrices = cache(
  async (projectId: string): Promise<ModelPrice[] | null> =>
    valueOf(await readLane("fetchModelPrices", projectId, "/prices", ModelPricesSchema))?.prices ??
    null,
);
