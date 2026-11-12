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
  StarterAlerts:         "starter_alerts",
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

/** Wire vocabulary of the Rust `SpanKind`: what one span of a trace is. */
export const SpanKind = {
  Agent:      "agent",
  TraceStart: "trace_start",
  Step:      "step",
  ModelCall: "model_call",
  ToolCall:  "tool_call",
  Retrieval: "retrieval",
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

/** Wire vocabulary of the Rust `ChangeKind`: what an agent's change changed. */
export const ChangeKind = {
  Release: "release",
  Model:   "model",
  Prompt:  "prompt",
  Tool:    "tool",
} as const;
export type ChangeKind = typeof ChangeKind[keyof typeof ChangeKind];

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
  Spend:        "spend",
  Duration:     "duration",
  TimeToFirstToken: "time_to_first_token",
} as const;
export type TraceSort = typeof TraceSort[keyof typeof TraceSort];

/** Wire vocabulary of the Rust `SessionSort`: a column of the sessions list, named in the URL. */
export const SessionSort = {
  LastSeen:     "last_seen",
  FirstSeen:    "first_seen",
  Traces:       "traces",
  FailedTraces: "failed_traces",
  Tokens:       "tokens",
  Spend:        "spend",
  Duration:     "duration",
} as const;
export type SessionSort = typeof SessionSort[keyof typeof SessionSort];

/** Wire vocabulary of the Rust `UserSort`: a column of the users list, named in the URL. */
export const UserSort = {
  LastSeen:     "last_seen",
  FirstSeen:    "first_seen",
  Sessions:     "sessions",
  Traces:       "traces",
  FailedTraces: "failed_traces",
  Tokens:       "tokens",
  Spend:        "spend",
  Duration:     "duration",
} as const;
export type UserSort = typeof UserSort[keyof typeof UserSort];

/** Wire vocabulary of the Rust `ScoreType`: what a score's value is. */
export const ScoreType = {
  Number:   "number",
  Boolean:  "boolean",
  Category: "category",
} as const;
export type ScoreType = typeof ScoreType[keyof typeof ScoreType];

/** Wire vocabulary of the Rust `ScoreSource`: who gave a score. */
export const ScoreSource = {
  Sdk:    "sdk",
  Member: "member",
} as const;
export type ScoreSource = typeof ScoreSource[keyof typeof ScoreSource];

/** Wire vocabulary of the Rust `ScoreTarget`: what a score judges. */
export const ScoreTarget = {
  Trace:   "trace",
  Span:    "span",
  Session: "session",
} as const;
export type ScoreTarget = typeof ScoreTarget[keyof typeof ScoreTarget];

/** Wire vocabulary of the Rust `SpanFault`: a class independent of status. */
export const SpanFault = {
  RateLimited: "rate_limited",
  Overloaded: "overloaded",
  Timeout: "timeout",
  ContextLength: "context_length",
  Unauthorized: "unauthorized",
  InvalidOutput: "invalid_output",
  Refused: "refused",
  Truncated: "truncated",
  Other: "other",
} as const;
export type SpanFault = typeof SpanFault[keyof typeof SpanFault];

/** Wire vocabulary of the Rust `MetricsGroup`: what a project's totals over
 *  time are split by. */
export const MetricsGroup = {
  Agent:       "agent",
  Model:       "model",
  Provider:    "provider",
  Tool:        "tool",
  Status:      "status",
  Fault:       "fault",
  Environment: "environment",
  Release:     "release",
  Tag:         "tag",
  User:        "user",
  Session:     "session",
} as const;
export type MetricsGroup = typeof MetricsGroup[keyof typeof MetricsGroup];

/** Wire vocabulary of the Rust `ChartMeasure`: what one of a dashboard's
 *  charts draws. */
export const ChartMeasure = {
  Spend:      "spend",
  Tokens:     "tokens",
  Traces:     "traces",
  Latency:    "latency",
  FirstToken: "first_token",
  OutputSpeed: "output_speed",
} as const;
export type ChartMeasure = typeof ChartMeasure[keyof typeof ChartMeasure];

/** Wire vocabulary of the Rust `RuleMeasure`: what an alert measures of each
 *  span it reads. */
export const RuleMeasure = {
  Count:    "count",
  Failures: "failures",
  Faults:   "faults",
  Duration: "duration",
  Tokens:   "tokens",
  Cost:     "cost",
  Score:    "score",
  Burn:     "burn",
  Steps:    "steps",
  Repeats:  "repeats",
  Stalled:  "stalled",
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
  Month: "month",
} as const;
export type RuleWindow = typeof RuleWindow[keyof typeof RuleWindow];

/** Wire vocabulary of the Rust `RuleBaseline`: what an alert's value is
 *  read against, itself or its change from the window before. */
export const RuleBaseline = {
  None:     "none",
  Previous: "previous",
} as const;
export type RuleBaseline = typeof RuleBaseline[keyof typeof RuleBaseline];

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

/** Wire vocabulary of the Rust `MonitorState`: where a monitor stands. */
export const MonitorState = {
  New:    "new",
  Up:     "up",
  Late:   "late",
  Down:   "down",
  Paused: "paused",
} as const;
export type MonitorState = typeof MonitorState[keyof typeof MonitorState];

/** Wire vocabulary of the Rust `MonitorCause`: why a monitor is late or down. */
export const MonitorCause = {
  Missed: "missed",
  Failed: "failed",
} as const;
export type MonitorCause = typeof MonitorCause[keyof typeof MonitorCause];

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
  Monitor:           "monitor",
  MaintenanceWindow: "maintenance_window",
  Incident:          "incident",
  Trace:             "trace",
  EndUser:           "end_user",
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

/** Telemetry's content fields (`contract::CONTENT`): what a person wrote or a
 *  model produced, kept only where a project keeps content, and drawn by the
 *  trace page in full. */
export const ContentField = {
  InputMessages:       "gen_ai.input.messages",
  OutputMessages:      "gen_ai.output.messages",
  SystemInstructions:  "gen_ai.system_instructions",
  ToolDefinitions:     "gen_ai.tool.definitions",
  ToolCallArguments:   "gen_ai.tool.call.arguments",
  ToolCallResult:      "gen_ai.tool.call.result",
  RetrievalQueryText:  "gen_ai.retrieval.query.text",
  RetrievalDocuments:  "gen_ai.retrieval.documents",
  MemoryQueryText:     "gen_ai.memory.query.text",
  MemoryRecords:       "gen_ai.memory.records",
  PromptVariable:      "gen_ai.prompt.variable",
  UserEmail:           "user.email",
  UserFullName:        "user.full_name",
  UserName:            "user.name",
  ContentCut:          "telmoni.content_cut",
  ExceptionType:       "exception.type",
  ExceptionMessage:    "exception.message",
  ExceptionStacktrace: "exception.stacktrace",
  StatusMessage:       "otel.status_description",
  Events:              "telmoni.events",
} as const;
export type ContentField = typeof ContentField[keyof typeof ContentField];

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

/** What a message's `file` part names a file this deployment keeps by, its
 *  hash after it — the Rust `content::FILE_ID_PREFIX`. */
export const Content = {
  fileIdPrefix: "telmoni:",
} as const;

/** The sentence a switched-off feature answers with, by flag — the Rust `Flag::off_detail`. */
export const FlagOffDetail = {
  api_tokens: "API keys are switched off right now.",
  connectors: "Connectors are switched off right now.",
  members:    "Adding a member is switched off right now.",
  public_api: "The API is switched off right now.",
  signup:     "Sign-ups are closed right now.",
} as const satisfies Record<Flag, string>;
