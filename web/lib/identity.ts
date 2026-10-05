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
/// else by address, the way a member row names a person — admins can change a
/// role too, but the owner is the one person every organization is sure to
/// have. `null` when `/me` did not carry the owner, as an entry read
/// mid-transfer does not, and the card falls back to "the owner".
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

// ⚠ **An organization is named by the name it was given — never by a
// person's name, and never by the owner's address.** `personName` above
// answers "which human is this", which is the right question on a member row
// and the wrong one on an organization row: an organization you were added to
// would render as its owner's name, and an address is a person's to show.
// Every organization has its name from birth, so this is that name; it stays
// one function because a console built on this one labels organizations
// through it (`lib/extension/ui.ts`).
export function organizationLabel(org: { name: string }): string {
  return org.name;
}

/// The organizations a person owns, as the account deletion names them:
/// counted by label, not listed, since two may carry one name.
export function ownedOrganizationLabels(
  organizations: readonly {
    name: string;
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
