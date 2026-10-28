import { cache } from "react";
import { z } from "zod";

import { valueOf } from "./read";
import { readLane } from "./traces";

// The field inventory: every attribute ingest reads, the spans it is read on,
// what becomes of it, the content mode it needs, and its rule. The server is
// the one source, so a row it adds needs no change here; `on` and `needs` are
// its words, shown as they come.
const InventoryRowSchema = z.object({
  attribute: z.string(),
  on: z.string(),
  kept: z.string(),
  needs: z.string(),
  rule: z.string(),
});

const FieldInventorySchema = z.object({
  conventions_commit: z.string(),
  rows: z.array(InventoryRowSchema),
});
export type FieldInventory = z.infer<typeof FieldInventorySchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads the inventory, so no refusal is expected here.
export const fetchFieldInventory = cache(
  async (projectId: string): Promise<FieldInventory | null> => {
    return valueOf(
      await readLane("fetchFieldInventory", projectId, "/field-inventory", FieldInventorySchema),
    );
  },
);
