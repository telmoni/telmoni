import { notFound } from "next/navigation";
import { z } from "zod";

import { Flag, FlagOffDetail, allOff, flagOn } from "@/lib/flags";

import { getServerContext } from "./entities/organization";

export async function requireFeature(...flags: readonly Flag[]): Promise<void> {
  const ctx = await getServerContext();
  if (ctx && allOff(ctx.flags, flags)) notFound();
}

const UNVERIFIED_TENANT =
  "We couldn't verify your organization just now. Try again shortly.";

export async function featureOff(flag: Flag): Promise<string | null> {
  const ctx = await getServerContext();
  if (!ctx) return UNVERIFIED_TENANT;
  if (!flagOn(ctx.flags, Flag.BetaAccess)) return FlagOffDetail[Flag.BetaAccess];
  return flagOn(ctx.flags, flag) ? null : FlagOffDetail[flag];
}

export const FeatureOffProblemSchema = z.object({
  type: z.literal("/errors/tenant/feature-off"),
  flag: z.string(),
});
