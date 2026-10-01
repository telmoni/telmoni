import { organizationLabel } from "@/lib/identity";

/**
 * Which organization you are standing in, and what to call it.
 *
 * ⚠ **ONE FALLBACK RULE, AND IT IS `organizationLabel`'s: the name the owner
 * chose, else the owner's address.** Every organization is an entry in
 * `/me`'s list, yours being the ones where your role is `owner`.
 */
export function resolveActiveOrganization<
  O extends { organizationId: string; name?: string | null; ownerEmail?: string | null },
>(args: {
  activeOrganizationId: string | null;
  organizations: readonly O[];
}): {
  active: O | null;
  label: string;
} {
  const { activeOrganizationId, organizations } = args;
  const active =
    organizations.find((o) => o.organizationId === activeOrganizationId) ?? null;
  return { active, label: active ? organizationLabel(active) : "Organization" };
}
