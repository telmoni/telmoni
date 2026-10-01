export function personName(who: {
  displayName?: string | null;
  email: string;
}): string {
  const named = who.displayName?.trim();
  return named ? named : who.email;
}

export function personDetail(who: {
  displayName?: string | null;
  email: string;
}): string | null {
  return personName(who) === who.email ? null : who.email;
}

/// Whom a refusal tells the caller to ask: the organization's owner by name,
/// else by address, the way a member row names a person — at either level
/// the owner is the one person who can change a role. `null` when `/me` did
/// not carry the owner, as an entry read mid-transfer does not, and the card
/// falls back to "the owner".
export function ownerContact(
  organization:
    | { ownerEmail?: string | null; ownerDisplayName?: string | null }
    | null
    | undefined,
): string | null {
  if (!organization?.ownerEmail) return null;
  return personName({
    displayName: organization.ownerDisplayName,
    email: organization.ownerEmail,
  });
}

// ⚠ **An organization is named by the name its owner gave it, else by its
// owner's ADDRESS — never by a person's name.** `personName` above answers
// "which human is this", which is the right question on a member row and the
// wrong one on an organization row. Both questions were asked of the same
// function, so an organization you had been added to rendered as its owner's
// name — a string that appears nowhere in the invitation, nowhere in the URL,
// nowhere in the audit log, and again two rows below on that same person's
// entry in Members.
//
// `name` is the organization's OWN name, set by its owner on the settings page,
// and it is the only thing that outranks the address. The address is the
// CURRENT owner's, which is why auth refuses to hand an unnamed organization
// over: its label would change hands with it. The last fallback covers an
// entry read mid-transfer, which has no owner row.
export function organizationLabel(org: {
  name?: string | null;
  ownerEmail?: string | null;
}): string {
  return org.name?.trim() || org.ownerEmail || "Organization";
}

/// The organizations a person owns, as the account deletion names them:
/// counted by label, not listed. Every unnamed organization is labelled by its
/// owner's address — this person's — so two of them would read as one
/// organization named twice.
export function ownedOrganizationLabels(
  organizations: readonly {
    name?: string | null;
    ownerEmail?: string | null;
    role: string;
  }[],
): string[] {
  const byLabel = new Map<string, number>();
  for (const o of organizations) {
    if (o.role !== "owner") continue;
    const label = organizationLabel(o);
    byLabel.set(label, (byLabel.get(label) ?? 0) + 1);
  }
  return [...byLabel].map(([label, count]) =>
    count > 1 ? `${label} (${count} organizations)` : label,
  );
}
