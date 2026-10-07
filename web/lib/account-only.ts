/// The (app) layout's gate: whether a signed-in person gets only their
/// account — their invitations, their account's deletion and sign-out —
/// rather than the console, because they are in no organization.
///
/// ⚠ **A gate closes the product, never the way out.** Closed sign-ups stop
/// somebody using Telmoni; they may not stop them accepting an invitation or
/// deleting their account, which the privacy policy promises they can do
/// without contacting anyone.
export function accountOnly(ctx: { activeOrganizationId: string | null }): boolean {
  return ctx.activeOrganizationId === null;
}
