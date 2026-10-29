// The wire vocabularies the console shares with the server. Written by hand;
// `contract.test.ts` fails if any differs from contract/wire-contract.json,
// which `make contract` writes from every module's Rust vocabularies.

/** Wire vocabulary of the Rust `ApiKeyScope`: what an API key opens. */
export const ApiKeyScope = {
  Read:   "read",
  Ingest: "ingest",
} as const;
export type ApiKeyScope = typeof ApiKeyScope[keyof typeof ApiKeyScope];

/** Wire vocabulary of the Rust `ContentMode`: what a project keeps of its spans' content. */
export const ContentMode = {
  Off: "off",
  On:  "on",
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

/** Wire vocabulary of the Rust `NotificationKind`. */
export const NotificationKind = {
  OrganizationAlert:     "organization_alert",
  MemberAdded:           "member_added",
  MemberLeft:            "member_left",
  ConnectorConnected:    "connector_connected",
  ConnectorDisconnected: "connector_disconnected",
  IncidentOpened:        "incident_opened",
  IncidentResolved:      "incident_resolved",
} as const;
export type NotificationKind = typeof NotificationKind[keyof typeof NotificationKind];

/** Wire vocabulary of the Rust `OrganizationStatus`. */
export const OrganizationStatus = {
  Active:          "active",
  PendingDeletion: "pending_deletion",
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
  Environment:      "environment",
  Release:          "release",
  Status:           "status",
  ErrorType:        "error_type",
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
  User:         "user",
  Traces:       "traces",
  Errors:       "errors",
  Steps:        "steps",
  InputTokens:  "input_tokens",
  OutputTokens: "output_tokens",
  Tokens:       "tokens",
  Duration:     "duration",
  FirstSeen:    "first_seen",
  LastSeen:     "last_seen",
} as const;
export type UserSort = typeof UserSort[keyof typeof UserSort];

/** Wire vocabulary of the Rust `UserColumn`: a column of the users page a
 *  person may hide, named in a saved view. */
export const UserColumn = {
  Traces:       "traces",
  Errors:       "errors",
  Steps:        "steps",
  InputTokens:  "input_tokens",
  OutputTokens: "output_tokens",
  Tokens:       "tokens",
  Duration:     "duration",
  FirstSeen:    "first_seen",
  LastSeen:     "last_seen",
} as const;
export type UserColumn = typeof UserColumn[keyof typeof UserColumn];

/** Wire vocabulary of the Rust `SessionSort`: a column of the sessions page, named in the URL. */
export const SessionSort = {
  Session:      "session",
  User:         "user",
  Traces:       "traces",
  Errors:       "errors",
  Steps:        "steps",
  InputTokens:  "input_tokens",
  OutputTokens: "output_tokens",
  Tokens:       "tokens",
  Duration:     "duration",
  FirstSeen:    "first_seen",
  LastSeen:     "last_seen",
} as const;
export type SessionSort = typeof SessionSort[keyof typeof SessionSort];

/** Wire vocabulary of the Rust `SessionColumn`: a column of the sessions page
 *  a person may hide, named in a saved view. */
export const SessionColumn = {
  User:         "user",
  Traces:       "traces",
  Errors:       "errors",
  Steps:        "steps",
  InputTokens:  "input_tokens",
  OutputTokens: "output_tokens",
  Tokens:       "tokens",
  Duration:     "duration",
  FirstSeen:    "first_seen",
  LastSeen:     "last_seen",
} as const;
export type SessionColumn = typeof SessionColumn[keyof typeof SessionColumn];

/** Wire vocabulary of the Rust `LayoutDepth`: how far a trace opens. */
export const LayoutDepth = {
  Trace: "trace",
  Steps: "steps",
  Calls: "calls",
} as const;
export type LayoutDepth = typeof LayoutDepth[keyof typeof LayoutDepth];

/** Wire vocabulary of the Rust `LayoutOpen`: which traces, and which
 *  branches of them, start open. */
export const LayoutOpen = {
  None:   "none",
  Failed: "failed",
  All:    "all",
} as const;
export type LayoutOpen = typeof LayoutOpen[keyof typeof LayoutOpen];

/** Wire vocabulary of the Rust `ViewKind`: what a saved view is a view of. */
export const ViewKind = {
  Traces:   "traces",
  Sessions: "sessions",
  Users:    "users",
  Layout:   "layout",
} as const;
export type ViewKind = typeof ViewKind[keyof typeof ViewKind];

/** Wire vocabulary of the Rust `RuleMeasure`: what an alert measures of each
 *  span it reads. */
export const RuleMeasure = {
  Count:    "count",
  Failures: "failures",
  Duration: "duration",
  Tokens:   "tokens",
  Cost:     "cost",
} as const;
export type RuleMeasure = typeof RuleMeasure[keyof typeof RuleMeasure];

/** Wire vocabulary of the Rust `RuleAggregation`: how an alert folds its
 *  measure over its window. */
export const RuleAggregation = {
  Count: "count",
  Avg:   "avg",
  Sum:   "sum",
  Max:   "max",
  P50:   "p50",
  P90:   "p90",
  P95:   "p95",
  P99:   "p99",
} as const;
export type RuleAggregation = typeof RuleAggregation[keyof typeof RuleAggregation];

/** Wire vocabulary of the Rust `RuleWindow`: how far back an alert reads. */
export const RuleWindow = {
  Hour: "hour",
  Day:  "day",
  Week: "week",
} as const;
export type RuleWindow = typeof RuleWindow[keyof typeof RuleWindow];

/** Wire vocabulary of the Rust `RuleOperator`: how an alert's value is
 *  compared with its thresholds. */
export const RuleOperator = {
  Gt:  "gt",
  Gte: "gte",
  Lt:  "lt",
  Lte: "lte",
  Eq:  "eq",
  Ne:  "ne",
} as const;
export type RuleOperator = typeof RuleOperator[keyof typeof RuleOperator];

/** Wire vocabulary of the Rust `MissingData`: what an alert does with a
 *  window that holds no span. */
export const MissingData = {
  Zero:         "zero",
  Keep:         "keep",
  NoData:       "no_data",
  NotifyNoData: "notify_no_data",
} as const;
export type MissingData = typeof MissingData[keyof typeof MissingData];

/** Wire vocabulary of the Rust `Severity`: where an alert stands. */
export const Severity = {
  Unknown: "unknown",
  Ok:      "ok",
  Warning: "warning",
  Alert:   "alert",
  NoData:  "no_data",
  Paused:  "paused",
} as const;
export type Severity = typeof Severity[keyof typeof Severity];

/** Wire vocabulary of the Rust `IncidentStatus`: where an incident stands. */
export const IncidentStatus = {
  Open:         "open",
  Acknowledged: "acknowledged",
  Resolved:     "resolved",
} as const;
export type IncidentStatus = typeof IncidentStatus[keyof typeof IncidentStatus];

/** Wire vocabulary of the Rust `RuleLevel`: which of an alert's notices a
 *  connector it links hears. */
export const RuleLevel = {
  Warning: "warning",
  Alert:   "alert",
} as const;
export type RuleLevel = typeof RuleLevel[keyof typeof RuleLevel];

/** Wire vocabulary of the Rust `SortDirection`: which way a list is sorted. */
export const SortDirection = {
  Asc:  "asc",
  Desc: "desc",
} as const;
export type SortDirection = typeof SortDirection[keyof typeof SortDirection];

/** Wire vocabulary of the Rust `OrganizationRole`: a person's place in an organization. */
export const OrganizationRole = {
  Owner:  "owner",
  Admin:  "admin",
  Member: "member",
} as const;
export type OrganizationRole = typeof OrganizationRole[keyof typeof OrganizationRole];

/** Wire vocabulary of the Rust `TelmoniResourceKind`: what an audit row names. */
export const ResourceKind = {
  Organization:      "organization",
  Project:           "project",
  Member:            "member",
  Token:             "token",
  Session:           "session",
  Connector:         "connector",
  Alert:             "alert",
  MaintenanceWindow: "maintenance_window",
  Incident:          "incident",
} as const;
export type ResourceKind = typeof ResourceKind[keyof typeof ResourceKind];

/** Wire vocabulary of the Rust `PausedBy`: a member, by hand, or the plan. */
export const PausedBy = {
  Person: "person",
  Plan:   "plan",
} as const;
export type PausedBy = typeof PausedBy[keyof typeof PausedBy];

/** Wire vocabulary of the Rust connector `Provider`. */
export const ConnectorProvider = {
  Slack:   "slack",
  Webhook: "webhook",
} as const;
export type ConnectorProvider = typeof ConnectorProvider[keyof typeof ConnectorProvider];

/** Wire vocabulary of the Rust `ConnectionStatus`: whether a connection hears notices. */
export const ConnectionStatus = {
  Active:  "active",
  Errored: "errored",
  Revoked: "revoked",
} as const;
export type ConnectionStatus = typeof ConnectionStatus[keyof typeof ConnectionStatus];

/** Wire vocabulary of the Rust `DeliveryStatus`: where one queued notice stands. */
export const DeliveryStatus = {
  Pending:   "pending",
  Delivered: "delivered",
  Failed:    "failed",
} as const;
export type DeliveryStatus = typeof DeliveryStatus[keyof typeof DeliveryStatus];

/** Wire vocabulary of the Rust `AttemptTrigger`: what made one delivery attempt. */
export const AttemptTrigger = {
  Scheduled: "scheduled",
  Manual:    "manual",
} as const;
export type AttemptTrigger = typeof AttemptTrigger[keyof typeof AttemptTrigger];

/** Wire vocabulary of the Rust `AttemptOutcome`: how one delivery attempt went. */
export const AttemptOutcome = {
  Delivered: "delivered",
  Failed:    "failed",
} as const;
export type AttemptOutcome = typeof AttemptOutcome[keyof typeof AttemptOutcome];

/** Wire vocabulary of the Rust `ExportFormat`: an audit export's file. */
export const ExportFormat = {
  Json: "json",
  Csv:  "csv",
} as const;
export type ExportFormat = typeof ExportFormat[keyof typeof ExportFormat];

/** Wire vocabulary of the Rust `ExportStatus`: where an audit export stands. */
export const ExportStatus = {
  Queued:  "queued",
  Running: "running",
  Ready:   "ready",
  Failed:  "failed",
} as const;
export type ExportStatus = typeof ExportStatus[keyof typeof ExportStatus];

/** Wire vocabulary of the Rust `ExportFailure`: why an audit export failed. */
export const ExportFailure = {
  TooLarge: "too_large",
  Error:    "error",
} as const;
export type ExportFailure = typeof ExportFailure[keyof typeof ExportFailure];

/** Wire vocabulary of the Rust `InviteScope`: which roster an invitation seats on. */
export const InviteScope = {
  Project:      "project",
  Organization: "organization",
} as const;
export type InviteScope = typeof InviteScope[keyof typeof InviteScope];

/** The agent's tools, by the names the model calls them by (the Rust `Tool`). */
export const AgentTool = {
  Search:              "search",
  ListMembers:         "list_members",
  ListConnectors:      "list_connectors",
  ConnectorDeliveries: "connector_deliveries",
  AuditEvents:         "audit_events",
} as const;
export type AgentTool = typeof AgentTool[keyof typeof AgentTool];

/** Wire vocabulary of the Rust agent `MessageRole`: who wrote a message. */
export const AgentMessageRole = {
  User:      "user",
  Assistant: "assistant",
} as const;
export type AgentMessageRole = typeof AgentMessageRole[keyof typeof AgentMessageRole];

/** Wire vocabulary of the Rust `TurnEventKind`: a turn's stream event names. */
export const AgentTurnEvent = {
  Conversation: "conversation",
  Text:         "text",
  Tool:         "tool",
  Citation:     "citation",
  Done:         "done",
  Error:        "error",
} as const;
export type AgentTurnEvent = typeof AgentTurnEvent[keyof typeof AgentTurnEvent];

/** The live channel's spellings, as the server publishes on them (`live.rs`). */
export const Live = {
  organizationChannel: "bfev:organization:",
  spansIngested:       "spans:ingested",
} as const;

/** The sentence a switched-off feature answers with, by flag — the Rust `Flag::off_detail`. */
export const FlagOffDetail = {
  api_tokens: "API keys are switched off right now.",
  connectors: "Connectors are switched off right now.",
  members:    "Adding a member is switched off right now.",
  public_api: "The API is switched off right now.",
  signup:     "Sign-ups are closed right now.",
} as const satisfies Record<Flag, string>;
