import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it, vi } from "vitest";
import { z } from "zod";

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server" } }));
import { parseAgentEvent } from "@/lib/agent/stream";
import {
  AuthnResultSchema,
  AuthorizeUrlSchema,
  LogoutUrlSchema,
  SignInCodeSchema,
  SignInConfigSchema,
} from "@/lib/auth/oidc";
import {
  InviteSentSchema,
  OfferChangeSchema,
  ProjectMovedSchema,
  RosterChangeSchema,
} from "@/lib/server/roster";
import { AuditExportListSchema, AuditExportSchema } from "@/lib/types/audit-export";
import { SettledInviteSchema } from "@/lib/types/settled-invite";

import { ActiveSessionListSchema } from "./active-sessions";
import { ConversationListSchema, ConversationSchema, StatusSchema } from "./agent";
import { AlertListSchema, IncidentListSchema, MuteListSchema } from "./alerts";
import { AuditPageSchema } from "./audit";
import { InviteListSchema, InviteLookSchema, MemberListSchema } from "./member";
import {
  AccountDeletedSchema,
  CreatedOrganizationSchema,
  EmailChangedSchema,
  MeSchema,
  OrganizationDeletedSchema,
  OrganizationNamedSchema,
  SeatSeenSchema,
} from "./organization";
import { OrganizationInviteListSchema, OrganizationMemberListSchema } from "./organization-member";
import {
  ProjectEverywhereListSchema,
  ProjectListSchema,
  ProjectRenamedSchema,
  ProjectSchema,
} from "./projects";
import { ApiTokenListSchema, MintedTokenSchema } from "./token";
import {
  AuthorizeSchema,
  ConnectionAnswerSchema,
  ConnectorListingSchema,
  DeliveryLogPageSchema,
  SendAnswerSchema,
  SigningSecretSchema,
} from "./connectors";
import { ContentModeSettingSchema } from "./content-mode";
import { FieldInventorySchema } from "./field-inventory";
import { FeedSchema } from "./notification-feed";
import { OrganizationHealthSchema } from "./organization-health";
import { SessionLayoutSettingSchema } from "./session-layout";
import { SessionDetailSchema, SessionListSchema } from "./sessions";
import {
  FacetsSchema,
  SavedViewOfAnyKindSchema,
  SessionViewListSchema,
  TraceDetailSchema,
  TraceHistogramSchema,
  TraceLayoutListSchema,
  TraceListSchema,
  TraceViewListSchema,
} from "./traces";
import { UserListSchema } from "./users";

// A turn's stream, each frame read as the agent panel's parser reads it.
const TurnFramesSchema = z.array(
  z
    .object({ event: z.string(), data: z.unknown() })
    .refine(({ event, data }) => parseAgentEvent(event, JSON.stringify(data)) !== null),
);

// Every answer the server's suites pin under `contract/responses`, written
// from its struct (`make contract`), by the schema the console reads it with.
const READERS: Record<string, z.ZodType> = {
  "agent/conversation.json": ConversationSchema,
  "agent/conversations.json": ConversationListSchema,
  "agent/status.json": StatusSchema,
  "agent/turn.json": TurnFramesSchema,
  "auth/account-deleted.json": AccountDeletedSchema,
  "auth/audit-export.json": AuditExportSchema,
  "auth/audit-exports.json": AuditExportListSchema,
  "auth/audit.json": AuditPageSchema,
  "auth/authn.json": AuthnResultSchema,
  "auth/config.json": SignInConfigSchema,
  "auth/declined.json": ProjectMovedSchema,
  "auth/email-changed.json": EmailChangedSchema,
  "auth/invite-created.json": InviteSentSchema,
  "auth/invite-look.json": InviteLookSchema,
  "auth/invite-revoked.json": RosterChangeSchema,
  "auth/invites.json": InviteListSchema,
  "auth/logout-url.json": LogoutUrlSchema,
  "auth/me.json": MeSchema,
  "auth/members.json": MemberListSchema,
  "auth/offer.json": OfferChangeSchema,
  "auth/organization-created.json": CreatedOrganizationSchema,
  "auth/organization-deleted.json": OrganizationDeletedSchema,
  "auth/organization-invites.json": OrganizationInviteListSchema,
  "auth/organization-members.json": OrganizationMemberListSchema,
  "auth/organization-named.json": OrganizationNamedSchema,
  "auth/project-accepted.json": ProjectMovedSchema,
  "auth/project-renamed.json": ProjectRenamedSchema,
  "auth/project.json": ProjectSchema,
  "auth/projects-everywhere.json": ProjectEverywhereListSchema,
  "auth/projects.json": ProjectListSchema,
  "auth/seat-given.json": RosterChangeSchema,
  "auth/seat-seen.json": SeatSeenSchema,
  "auth/seated.json": SettledInviteSchema,
  "auth/sessions.json": ActiveSessionListSchema,
  "auth/settled-invite.json": SettledInviteSchema,
  "auth/sign-in.json": SignInCodeSchema,
  "auth/start.json": AuthorizeUrlSchema,
  "auth/token-minted.json": MintedTokenSchema,
  "auth/tokens.json": ApiTokenListSchema,
  "notifications/authorize.json": AuthorizeSchema,
  "notifications/connection.json": ConnectionAnswerSchema,
  "notifications/connectors.json": ConnectorListingSchema,
  "notifications/deliveries.json": DeliveryLogPageSchema,
  "notifications/feed.json": FeedSchema,
  "notifications/sent.json": SendAnswerSchema,
  "notifications/signing-secret.json": SigningSecretSchema,
  "telemetry/alerts.json": AlertListSchema,
  "telemetry/content-mode.json": ContentModeSettingSchema,
  "telemetry/facets.json": FacetsSchema,
  "telemetry/field-inventory.json": FieldInventorySchema,
  "telemetry/health.json": OrganizationHealthSchema,
  "telemetry/histogram.json": TraceHistogramSchema,
  "telemetry/incidents.json": IncidentListSchema,
  "telemetry/layouts.json": TraceLayoutListSchema,
  "telemetry/mutes.json": MuteListSchema,
  "telemetry/session-layout.json": SessionLayoutSettingSchema,
  "telemetry/session-views.json": SessionViewListSchema,
  "telemetry/session.json": SessionDetailSchema,
  "telemetry/sessions.json": SessionListSchema,
  "telemetry/trace.json": TraceDetailSchema,
  "telemetry/traces.json": TraceListSchema,
  "telemetry/users.json": UserListSchema,
  "telemetry/views.json": TraceViewListSchema,
};

// Readers that take every field as optional, so a key renamed would read as
// absent: each key they read is looked for by name.
const READ_BY_NAME: Record<string, readonly string[]> = {
  "auth/declined.json": ["ownerOrganizationId"],
  "auth/invite-created.json": ["id", "link", "expiresAt", "ownerOrganizationId"],
  "auth/invite-revoked.json": ["email", "ownerOrganizationId"],
  "auth/offer.json": ["offeredToEmail", "withdrawnFromEmail"],
  "auth/project-accepted.json": ["slug", "organizationId", "organizationSlug", "previousOrganizationId"],
  "auth/seat-given.json": ["email", "ownerOrganizationId"],
  "auth/seated.json": ["inviteId", "inviterEmail", "ownerOrganizationId"],
  "auth/settled-invite.json": ["inviteId", "inviterEmail", "ownerOrganizationId"],
};

const PINNED = fileURLToPath(new URL("../../../../contract/responses/", import.meta.url));

function pinned(name: string): unknown {
  return JSON.parse(readFileSync(`${PINNED}${name}`, "utf8"));
}

describe("the server's answers", () => {
  it.each(Object.entries(READERS))("%s reads", (name, schema) => {
    expect(schema.safeParse(pinned(name)).error).toBeUndefined();
  });

  it.each(Object.entries(READ_BY_NAME))("%s carries each key its reader takes", (name, keys) => {
    expect(Object.keys(pinned(name) as object)).toEqual(expect.arrayContaining([...keys]));
  });

  it("each has a reader", () => {
    const files = readdirSync(PINNED, { recursive: true, encoding: "utf8" })
      .filter((file) => file.endsWith(".json"))
      .sort();
    expect(files).toEqual(Object.keys(READERS).sort());
  });

  it("a saved view of every kind reads as a save answers it", () => {
    const Views = z.object({ views: z.array(z.unknown()) });
    for (const name of ["telemetry/views.json", "telemetry/session-views.json", "telemetry/layouts.json"]) {
      for (const view of Views.parse(pinned(name)).views) {
        expect(SavedViewOfAnyKindSchema.safeParse(view).error).toBeUndefined();
      }
    }
  });
});
