// The wire vocabularies the console shares with the server. Written by hand;
// `contract.test.ts` fails if any differs from contract/wire-contract.json,
// which `make contract` writes from the Rust enums in crates/shared/src/types.

/** Wire vocabulary of the Rust `Flag`. */
export const Flag = {
  Connectors: "connectors",
  PublicApi:  "public_api",
  ApiTokens:  "api_tokens",
  Members:    "members",
  Signup:     "signup",
  BetaAccess: "beta_access",
} as const;
export type Flag = typeof Flag[keyof typeof Flag];

/** Wire vocabulary of the Rust `NotificationKind`. */
export const NotificationKind = {
  OrganizationAlert:     "organization_alert",
  MemberAdded:           "member_added",
  ConnectorConnected:    "connector_connected",
  ConnectorDisconnected: "connector_disconnected",
} as const;
export type NotificationKind = typeof NotificationKind[keyof typeof NotificationKind];

/** Wire vocabulary of the Rust `OrganizationStatus`. */
export const OrganizationStatus = {
  Active:          "active",
  PendingDeletion: "pending_deletion",
  Deleted:         "deleted",
} as const;
export type OrganizationStatus = typeof OrganizationStatus[keyof typeof OrganizationStatus];

/** Wire vocabulary of the Rust `Role`. */
export const Role = {
  Owner:  "owner",
  Admin:  "admin",
  Member: "member",
} as const;
export type Role = typeof Role[keyof typeof Role];

/** The sentence a switched-off feature answers with, by flag — the Rust `Flag::off_detail`. */
export const FlagOffDetail = {
  api_tokens:  "API keys are switched off right now.",
  beta_access: "This organization is not in the beta yet.",
  connectors:  "Connectors are switched off right now.",
  members:     "Adding a member is switched off right now.",
  public_api:  "The API is switched off right now.",
  signup:      "Sign-ups are closed right now.",
} as const satisfies Record<Flag, string>;

const ROLES: ReadonlySet<string> = new Set(Object.values(Role));

export function asRole(value: string | null | undefined): Role | null {
  return value && ROLES.has(value) ? (value as Role) : null;
}
