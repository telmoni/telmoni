import { organizationLabel } from "@/lib/identity";

const collator = new Intl.Collator("en", { sensitivity: "base" });

export function compareProjects(a: { name: string }, b: { name: string }): number {
  return collator.compare(a.name, b.name);
}

type OrganizationProject = {
  name: string;
  organizationName: string | null;
  organizationOwnerEmail: string | null;
};

// Grouped by what the switcher prints for each organization — the rule
// `organizationLabel` spells — so a group sorts where its heading reads.
export function compareProjectsByOrganization(
  a: OrganizationProject,
  b: OrganizationProject,
): number {
  const byOrganization = collator.compare(
    organizationLabel({ name: a.organizationName, ownerEmail: a.organizationOwnerEmail }),
    organizationLabel({ name: b.organizationName, ownerEmail: b.organizationOwnerEmail }),
  );
  return byOrganization !== 0 ? byOrganization : compareProjects(a, b);
}
