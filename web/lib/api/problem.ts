import { z } from "zod";

export const Problem = z
  .object({
    type: z.string().optional(),
    title: z.string(),
    status: z.number(),
    detail: z.string().optional(),
    instance: z.string().optional(),
  })
  .passthrough();

export type Problem = z.infer<typeof Problem>;

