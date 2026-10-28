import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  AgentMessageRole,
  AgentTool,
  AgentTurnEvent,
  ApiKeyScope,
  AttemptOutcome,
  AttemptTrigger,
  ConnectionStatus,
  ConnectorProvider,
  ContentMode,
  DeliveryStatus,
  ExportFailure,
  ExportFormat,
  ExportStatus,
  Flag,
  FlagOffDetail,
  IncidentStatus,
  InviteScope,
  LayoutDepth,
  LayoutOpen,
  Live,
  MissingData,
  NotificationKind,
  OrganizationRole,
  OrganizationStatus,
  PausedBy,
  ResourceKind,
  Role,
  RuleAggregation,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  SessionSort,
  Severity,
  SortDirection,
  SpanKind,
  SpanStatus,
  TraceColumn,
  TraceSort,
  UserSort,
  ViewKind,
} from "./enums";
import { AGENT_MESSAGE_MAX, AGENT_STREAM_TIMEOUT_MS } from "./agent";
import { AUDIT_EXPORT_KEPT_DAYS } from "./audit-export";
import {
  ALERT_KINDS,
  ALERT_NAME_MAX,
  INCIDENTS_MAX,
  LINKS_MAX,
  MUTE_MINUTES_MAX,
  RENOTIFY_MAX_MINUTES,
  aggregationsFor,
  measuresFor,
} from "../alerts";
import { CONFIRMATION_CODE_LENGTH } from "../confirmation-code";
import { PINNED_MAX, PINNED_PER_TRACE } from "../layout";
import { FILTER_VALUES_MAX, MAX_DAYS, MAX_LIMIT, OPENING_DAYS } from "../list-query";
import {
  MAX_API_KEY_DESCRIPTION,
  MAX_API_KEY_NAME,
  MAX_EMAIL,
  MAX_ORGANIZATION_NAME,
  MAX_PROJECT_NAME,
} from "../names";
import { RESERVED_ORGANIZATION_SLUGS, RESERVED_PROJECT_SLUGS, SLUG_MAX_LENGTH } from "../slug";
import { DOWNLOAD_MAX, FILTER_VALUE_MAX, MAX_PAGE, RUNS_MAX } from "../traces";
import { UTC_OFFSET_EAST, UTC_OFFSET_WEST } from "../utc-offset";

const contract = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../../../contract/wire-contract.json", import.meta.url)),
    "utf8",
  ),
) as {
  enums: Record<string, string[]>;
  flags: { off_detail: Record<string, string> };
  slugs: { max_length: number; reserved: { organization: string[]; project: string[] } };
  live: { organization_channel: string; spans_ingested: string };
  limits: { agent_stream_max_secs: number; [limit: string]: unknown };
  rules: { folds: Record<string, string[]>; measures: Record<string, string[]> };
};

// The one limit the console exceeds rather than copies.
const { agent_stream_max_secs: STREAM_MAX_SECS, ...LIMITS } = contract.limits;

// Every enum the server publishes, and the console's copy of it. A new one
// in the contract fails the first test until it is added here.
const CONSOLE: Record<string, Record<string, string>> = {
  AgentMessageRole,
  AgentTool,
  AgentTurnEvent,
  ApiKeyScope,
  AttemptOutcome,
  AttemptTrigger,
  ConnectionStatus,
  ConnectorProvider,
  ContentMode,
  DeliveryStatus,
  ExportFailure,
  ExportFormat,
  ExportStatus,
  Flag,
  IncidentStatus,
  InviteScope,
  LayoutDepth,
  LayoutOpen,
  MissingData,
  NotificationKind,
  OrganizationRole,
  OrganizationStatus,
  PausedBy,
  ResourceKind,
  Role,
  RuleAggregation,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  SessionSort,
  Severity,
  SortDirection,
  SpanKind,
  SpanStatus,
  TraceColumn,
  TraceSort,
  UserSort,
  ViewKind,
};

describe("wire contract", () => {
  it("the console knows every enum the server publishes", () => {
    expect(Object.keys(CONSOLE).sort()).toEqual(Object.keys(contract.enums).sort());
  });

  // In the server's order, not as sets: the console lists some in that order —
  // the content modes, a connector's events — and a copy holding the right
  // values shuffled would list them wrong.
  it.each(Object.keys(CONSOLE))("%s has exactly the server's values, in its order", (name) => {
    expect(Object.values(CONSOLE[name]!)).toEqual(contract.enums[name]);
  });

  it("each switched-off feature says what the server says", () => {
    expect(FlagOffDetail).toEqual(contract.flags.off_detail);
  });

  // The live lists redraw on this event alone, so a drift would stop them
  // without an error.
  it("listens where the server publishes, for the event it names", () => {
    expect(Live.organizationChannel).toBe(contract.live.organization_channel);
    expect(Live.spansIngested).toBe(contract.live.spans_ingested);
  });

  // The console checks these before it asks, so a copy below the server's
  // refuses what the server takes, and one above sends what it refuses.
  it("the console's limits are the server's", () => {
    expect({
      list_default_days: OPENING_DAYS,
      list_max_days: MAX_DAYS,
      list_max_limit: MAX_LIMIT,
      list_max_page: MAX_PAGE,
      list_export_max: DOWNLOAD_MAX,
      list_runs_max: RUNS_MAX,
      filter_values_max: FILTER_VALUES_MAX,
      filter_value_max: FILTER_VALUE_MAX,
      pinned_max: PINNED_MAX,
      pinned_per_run: PINNED_PER_TRACE,
      utc_offset_west: UTC_OFFSET_WEST,
      utc_offset_east: UTC_OFFSET_EAST,
      alert_name_max: ALERT_NAME_MAX,
      alert_renotify_max_minutes: RENOTIFY_MAX_MINUTES,
      alert_links_max: LINKS_MAX,
      mute_days_max: MUTE_MINUTES_MAX / 1_440,
      incidents_max: INCIDENTS_MAX,
      organization_name_max: MAX_ORGANIZATION_NAME,
      project_name_max: MAX_PROJECT_NAME,
      api_key_name_max: MAX_API_KEY_NAME,
      api_key_description_max: MAX_API_KEY_DESCRIPTION,
      email_max: MAX_EMAIL,
      agent_message_max: AGENT_MESSAGE_MAX,
      audit_export_kept_days: AUDIT_EXPORT_KEPT_DAYS,
      confirmation_code_length: CONFIRMATION_CODE_LENGTH,
    }).toEqual(LIMITS);
  });

  // The server ends a turn and saves it first; a console that cut the
  // stream sooner would lose the reply mid-sentence.
  it("the console waits on a turn longer than the server lets one run", () => {
    expect(AGENT_STREAM_TIMEOUT_MS).toBeGreaterThan(STREAM_MAX_SECS * 1_000);
  });

  // The form offers what the server folds, and nothing it refuses.
  it("the alert form folds each measure as the server does", () => {
    for (const measure of Object.values(RuleMeasure)) {
      expect([...aggregationsFor(measure)].sort()).toEqual([...contract.rules.folds[measure]!].sort());
    }
  });

  // The kinds an alert watches, and what the server counts on each.
  it("the alert form measures each kind as the server does", () => {
    expect(Object.keys(contract.rules.measures).sort()).toEqual([...ALERT_KINDS].sort());
    for (const kind of ALERT_KINDS) {
      expect([...measuresFor(kind)].sort()).toEqual([...contract.rules.measures[kind]!].sort());
    }
  });

  // The proxy reads a path's first segment as an organization unless it is
  // one of these; a word auth reserves and the console does not would be a
  // page of its own that the proxy took for an organization.
  it("the console reserves exactly the words the server refuses an organization", () => {
    expect([...RESERVED_ORGANIZATION_SLUGS].sort()).toEqual(
      [...contract.slugs.reserved.organization].sort(),
    );
    expect(SLUG_MAX_LENGTH).toBe(contract.slugs.max_length);
  });

  // The rail reads a path's second segment as an organization page when it is
  // one of these, and as a project otherwise; a word the two lists disagree on
  // is a project the console draws as a page, or a page it draws as a project.
  it("the console reserves exactly the words the server refuses a project", () => {
    expect([...RESERVED_PROJECT_SLUGS].sort()).toEqual([...contract.slugs.reserved.project].sort());
  });
});
