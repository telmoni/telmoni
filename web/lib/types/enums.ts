// The wire vocabularies the console shares with the server. Written by hand;
// `contract.test.ts` fails if any differs from contract/wire-contract.json,
// which `make contract` writes from the Rust enums in crates/shared/src/types.

/** Wire vocabulary of the Rust `ApiKeyScope`: what an API key opens. */
export const ApiKeyScope = {
  Read:   "read",
  Ingest: "ingest",
} as const;
export type ApiKeyScope = typeof ApiKeyScope[keyof typeof ApiKeyScope];

/** Wire vocabulary of the Rust `ContentMode`: what a project keeps of its spans' content. */
export const ContentMode = {
  Off:    "off",
  On:     "on",
  Sealed: "sealed",
} as const;
export type ContentMode = typeof ContentMode[keyof typeof ContentMode];

/** Wire vocabulary of the Rust `Flag`. */
export const Flag = {
  Connectors: "connectors",
  PublicApi:  "public_api",
  ApiTokens:  "api_tokens",
  Members:    "members",
  Signup:     "signup",
} as const;
export type Flag = typeof Flag[keyof typeof Flag];

/** Wire vocabulary of the Rust `MonitorState`: what a monitor is in, the worst first. */
export const MonitorState = {
  Failed:    "failed",
  Stalled:   "stalled",
  OverBound: "over_bound",
  Late:      "late",
  Running:   "running",
  New:       "new",
  Paused:    "paused",
  Healthy:   "healthy",
} as const;
export type MonitorState = typeof MonitorState[keyof typeof MonitorState];

/** Wire vocabulary of the Rust `NotificationKind`. */
export const NotificationKind = {
  OrganizationAlert:     "organization_alert",
  MemberAdded:           "member_added",
  MemberLeft:            "member_left",
  ConnectorConnected:    "connector_connected",
  ConnectorDisconnected: "connector_disconnected",
  MonitorDiscovered:     "monitor_discovered",
  PingKeyRetired:        "ping_key_retired",
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

/** Wire vocabulary of the Rust `SpanKind`: what one span of a run is. */
export const SpanKind = {
  Run:       "run",
  Step:      "step",
  ModelCall: "model_call",
  ToolCall:  "tool_call",
  Span:      "span",
} as const;
export type SpanKind = typeof SpanKind[keyof typeof SpanKind];

/** Wire vocabulary of the Rust `SpanStatus`: how a span ended. */
export const SpanStatus = {
  Ok:        "ok",
  Error:     "error",
  Cancelled: "cancelled",
} as const;
export type SpanStatus = typeof SpanStatus[keyof typeof SpanStatus];

/** Wire vocabulary of the Rust `TraceSort`: a column of the trace list, named in the URL. */
export const TraceSort = {
  Started:      "started",
  Agent:        "agent",
  User:         "user",
  Session:      "session",
  Status:       "status",
  Steps:        "steps",
  ModelCalls:   "model_calls",
  ToolCalls:    "tool_calls",
  Errors:       "errors",
  InputTokens:  "input_tokens",
  OutputTokens: "output_tokens",
  Tokens:       "tokens",
  Duration:     "duration",
} as const;
export type TraceSort = typeof TraceSort[keyof typeof TraceSort];

/** Wire vocabulary of the Rust `TraceColumn`: a column of the trace list a
 *  person shows or hides, named in a saved view. */
export const TraceColumn = {
  Started:          "started",
  Ended:            "ended",
  Agent:            "agent",
  User:             "user",
  Session:          "session",
  Status:           "status",
  Steps:            "steps",
  ModelCalls:       "model_calls",
  ToolCalls:        "tool_calls",
  Errors:           "errors",
  Spans:            "spans",
  InputTokens:      "input_tokens",
  OutputTokens:     "output_tokens",
  CacheReadTokens:  "cache_read_tokens",
  CacheWriteTokens: "cache_write_tokens",
  Tokens:           "tokens",
  Duration:         "duration",
  Models:           "models",
  Tools:            "tools",
} as const;
export type TraceColumn = typeof TraceColumn[keyof typeof TraceColumn];

/** Wire vocabulary of the Rust `UserSort`: a column of the users page, named in the URL. */
export const UserSort = {
  User:      "user",
  Traces:    "traces",
  Errors:    "errors",
  Steps:     "steps",
  Tokens:    "tokens",
  FirstSeen: "first_seen",
  LastSeen:  "last_seen",
} as const;
export type UserSort = typeof UserSort[keyof typeof UserSort];

/** Wire vocabulary of the Rust `SessionSort`: a column of the sessions page, named in the URL. */
export const SessionSort = {
  Session:   "session",
  User:      "user",
  Traces:    "traces",
  Errors:    "errors",
  Steps:     "steps",
  Tokens:    "tokens",
  FirstSeen: "first_seen",
  LastSeen:  "last_seen",
} as const;
export type SessionSort = typeof SessionSort[keyof typeof SessionSort];

/** Wire vocabulary of the Rust `SortDirection`: which way a list is sorted. */
export const SortDirection = {
  Asc:  "asc",
  Desc: "desc",
} as const;
export type SortDirection = typeof SortDirection[keyof typeof SortDirection];

/** The sentence a switched-off feature answers with, by flag — the Rust `Flag::off_detail`. */
export const FlagOffDetail = {
  api_tokens: "API keys are switched off right now.",
  connectors: "Connectors are switched off right now.",
  members:    "Adding a member is switched off right now.",
  public_api: "The API is switched off right now.",
  signup:     "Sign-ups are closed right now.",
} as const satisfies Record<Flag, string>;

const ROLES: ReadonlySet<string> = new Set(Object.values(Role));

export function asRole(value: string | null | undefined): Role | null {
  return value && ROLES.has(value) ? (value as Role) : null;
}
