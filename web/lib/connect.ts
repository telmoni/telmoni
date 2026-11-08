// The two connectors the console offers, and the one place the handshake's
// path shapes are spelled. Shared by the two Route Handlers under
// `app/connect/[provider]/` and the Connectors page that links to them.
//
// Slack is an OAuth grant the browser is handed off for; the webhook is one an
// owner or admin connects with a URL, in a dialog, and it never reaches the
// handshake routes — which is why they are two lists.

import { type SearchParams, one } from "@/lib/list-query";
import type { Connection } from "@/lib/server/data";
import { isOneOf } from "@/lib/one-of";
import { projectPath } from "@/lib/slug";
import { ConnectorProvider } from "@/lib/types/enums";

export const OAUTH_PROVIDERS = [ConnectorProvider.Slack] as const;
export type OAuthProvider = (typeof OAUTH_PROVIDERS)[number];

export const PROVIDERS = Object.values(ConnectorProvider);
export type Provider = ConnectorProvider;

export function isOAuthProvider(value: string): value is OAuthProvider {
  return isOneOf(OAUTH_PROVIDERS, value);
}

export const PROVIDER_LABEL: Record<Provider, string> = {
  slack: "Slack",
  webhook: "Webhook",
};

/** Where a connection points: `#alerts in Acme` for Slack; a webhook's name,
 *  or else its host, never the URL, whose path may carry the customer's token. */
export function connectionName(
  connection: Pick<Connection, "provider" | "channel_name" | "external_workspace_name">,
): string {
  if (connection.provider === "webhook") {
    return connection.external_workspace_name ?? connection.channel_name;
  }
  const channel = `#${connection.channel_name.replace(/^#/, "")}`;
  return connection.external_workspace_name
    ? `${channel} in ${connection.external_workspace_name}`
    : channel;
}

export function connectStartPath(provider: OAuthProvider, projectId: string): string {
  return `/connect/${provider}/start?project=${encodeURIComponent(projectId)}`;
}

/** Where a person starts linking their own Slack account, from their
 *  account settings: the handshake's other purpose, which names no project. */
export function identityStartPath(provider: OAuthProvider): string {
  return `/connect/${provider}/identity`;
}

/** Why linking a Slack account came back with nothing linked. */
export const LINK_REFUSALS = [
  "denied",
  "vendor",
  "callback",
  "elsewhere",
  "unavailable",
  "start",
] as const;
export type LinkRefusal = (typeof LINK_REFUSALS)[number];

/** How linking came back: linked, or why not. */
export type LinkOutcome = "linked" | LinkRefusal;

/** The account settings page, at its Slack section, with how a link came back. */
export function accountSlackPath(outcome?: LinkOutcome): string {
  return `/settings/account${outcome ? `?slack=${outcome}` : ""}#slack`;
}

/** The link outcome an account page was sent back with; a word no route
 *  writes reads as the plain failure. */
export function linkOutcome(params: SearchParams): LinkOutcome | null {
  const word = one(params.slack);
  if (word === undefined) return null;
  if (word === "linked") return word;
  return LINK_REFUSALS.find((refusal) => refusal === word) ?? "callback";
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
