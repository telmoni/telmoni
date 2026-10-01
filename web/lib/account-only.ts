import { Flag, type FlagSet, flagOn } from "@/lib/flags";

/// Why a signed-in person gets their account instead of the console.
export type AccountOnlyReason = "no-organization" | "not-in-beta";

/// The (app) layout's gate: `null` for the console, else why a signed-in
/// person gets only their account — their invitations, their account's
/// deletion and sign-out.
///
/// ⚠ **A gate closes the product, never the way out.** Closed sign-ups and
/// the beta wall both stop somebody using Telmoni; neither may stop them
/// accepting an invitation or deleting their account, which the privacy
/// policy promises they can do without contacting anyone.
export function accountOnlyReason(ctx: {
  activeOrganizationId: string | null;
  flags: FlagSet;
}): AccountOnlyReason | null {
  if (ctx.activeOrganizationId === null) return "no-organization";
  if (!flagOn(ctx.flags, Flag.BetaAccess)) return "not-in-beta";
  return null;
}
