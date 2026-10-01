import { z } from "zod";

import { Flag, FlagOffDetail } from "@/lib/types/enums";

export { Flag, FlagOffDetail };

export const FlagSetSchema = z.record(z.string(), z.boolean());
export type FlagSet = z.infer<typeof FlagSetSchema>;

export function flagOn(set: FlagSet | null | undefined, flag: Flag): boolean {
  return set?.[flag] ?? true;
}

export function allOff(set: FlagSet | null | undefined, flags: readonly Flag[]): boolean {
  return flags.length > 0 && flags.every((f) => !flagOn(set, f));
}
