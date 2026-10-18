export const OrganizationRole = {
  Owner: "owner",
  Admin: "admin",
  Member: "member",
} as const;

export type OrganizationRole = (typeof OrganizationRole)[keyof typeof OrganizationRole];

export function asOrganizationRole(value: unknown): OrganizationRole | null {
  if (typeof value !== "string") return null;
  const normalized = value.trim().toLowerCase();
  switch (normalized) {
    case "owner":
      return OrganizationRole.Owner;
    case "admin":
      return OrganizationRole.Admin;
    case "member":
      return OrganizationRole.Member;
    default:
      return null;
  }
}
