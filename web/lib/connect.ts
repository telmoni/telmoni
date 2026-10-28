// The two connectors the console offers, and the one place the handshake's
// path shapes are spelled. Shared by the two Route Handlers under
// `app/connect/[provider]/` and the Connectors page that links to them.
//
// Slack is an OAuth grant the browser is handed off for; the webhook is one an
// owner or admin connects with a URL, in a dialog, and it never reaches the
// handshake routes — which is why they are two lists.

import { type SearchParams, one } from "@/lib/list-query";
import type { Connection } from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { ConnectorProvider } from "@/lib/types/enums";

export const OAUTH_PROVIDERS = [ConnectorProvider.Slack] as const;
export type OAuthProvider = (typeof OAUTH_PROVIDERS)[number];

export const PROVIDERS = Object.values(ConnectorProvider);
export type Provider = ConnectorProvider;

export function isOAuthProvider(value: string): value is OAuthProvider {
  return (OAUTH_PROVIDERS as readonly string[]).includes(value);
}

export const PROVIDER_LABEL: Record<Provider, string> = {
  slack: "Slack",
  webhook: "Webhook",
};

/** Where a connection points, in the vendor's own terms: `#alerts in Acme`
 *  for Slack, and the host alone for a webhook, whose path may carry the
 *  customer's own token. */
export function connectionName(
  connection: Pick<Connection, "provider" | "channel_name" | "external_workspace_name">,
): string {
  if (connection.provider === "webhook") {
    return connection.channel_name;
  }
  const channel = `#${connection.channel_name.replace(/^#/, "")}`;
  return connection.external_workspace_name
    ? `${channel} in ${connection.external_workspace_name}`
    : channel;
}

// A project id as the console spells it, and as it may appear in a redirect
// target this route builds: no separators a path could misread.
const PROJECT_ID_RE = /^[A-Za-z0-9_-]{1,64}$/;

/** A Server Action takes whatever was posted, whatever its parameter's type
 *  says, and `RegExp.test` would read `["project_1"]` as its string. Not a
 *  type guard: a caller's `string` refused here would narrow to `never`. */
export function isProjectId(value: unknown): boolean {
  return typeof value === "string" && PROJECT_ID_RE.test(value);
}

export function connectStartPath(provider: OAuthProvider, projectId: string): string {
  return `/connect/${provider}/start?project=${encodeURIComponent(projectId)}`;
}

/** Why a handshake came back with nothing connected. */
export const CONNECT_REFUSALS = [
  "denied",
  "vendor",
  "callback",
  "forbidden",
  "off",
  "unavailable",
  "project",
  "authorize",
] as const;
export type ConnectRefusal = (typeof CONNECT_REFUSALS)[number];

/** How a handshake came back: one notice, never a success and a refusal. */
export type ConnectOutcome = { connected: OAuthProvider } | { error: ConnectRefusal };

// A project's Connectors page. By slug, or by id where the handshake has
// nothing else at hand: the console redirects an id to the slug it goes by.
export function connectorsPath(
  organization: string,
  project: string,
  outcome?: ConnectOutcome,
): string {
  const q = new URLSearchParams(outcome).toString();
  return `${projectPath(organization, project, "/connectors")}${q ? `?${q}` : ""}`;
}

/** The outcome a Connectors page was sent back with, read as a URL may carry
 *  it: a word no route writes reads as the plain failure. */
export function connectOutcome(params: SearchParams): ConnectOutcome | null {
  const connected = one(params.connected);
  if (connected && isOAuthProvider(connected)) return { connected };
  const error = one(params.error);
  if (error === undefined) return null;
  return { error: CONNECT_REFUSALS.find((refusal) => refusal === error) ?? "authorize" };
}
