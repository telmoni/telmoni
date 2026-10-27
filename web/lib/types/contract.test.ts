import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  AgentMessageRole,
  AgentTool,
  AgentTurnEvent,
  ApiKeyScope,
  asRole,
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
import { RESERVED_ORGANIZATION_SLUGS, RESERVED_PROJECT_SLUGS, SLUG_MAX_LENGTH } from "../slug";

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
};

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

  it("the role coercion takes the three roles and nothing else", () => {
    expect(asRole("editor")).toBeNull();
    expect(asRole("viewer")).toBeNull();
    expect(asRole("guest")).toBeNull();

    expect(asRole("owner")).toBe(Role.Owner);
    expect(asRole("admin")).toBe(Role.Admin);
    expect(asRole("member")).toBe(Role.Member);
  });
});
