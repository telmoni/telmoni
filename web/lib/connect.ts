// The three connectors the console offers, and the one place the handshake's
// path shapes are spelled. Shared by the two Route Handlers under
// `app/connect/[provider]/` and the Connectors page that links to them.
//
// Two of the three are OAuth grants the browser is handed off for; the third
// is a webhook the owner connects with a URL, in a dialog, and it never
// reaches the handshake routes — which is why they are two lists.

export const OAUTH_PROVIDERS = ["slack", "discord"] as const;
export type OAuthProvider = (typeof OAUTH_PROVIDERS)[number];

export const PROVIDERS = [...OAUTH_PROVIDERS, "webhook"] as const;
export type Provider = (typeof PROVIDERS)[number];

export function isProvider(value: string): value is Provider {
  return (PROVIDERS as readonly string[]).includes(value);
}

export function isOAuthProvider(value: string): value is OAuthProvider {
  return (OAUTH_PROVIDERS as readonly string[]).includes(value);
}

export const PROVIDER_LABEL: Record<Provider, string> = {
  slack: "Slack",
  discord: "Discord",
  webhook: "Webhook",
};

// A project id as the console spells it, and as it may appear in a redirect
// target this route builds: no separators a path could misread.
const PROJECT_ID_RE = /^[A-Za-z0-9_-]{1,64}$/;

export function isProjectId(value: string): boolean {
  return PROJECT_ID_RE.test(value);
}

export function connectStartPath(provider: OAuthProvider, projectId: string): string {
  return `/connect/${provider}/start?project=${encodeURIComponent(projectId)}`;
}

export function connectorsPath(
  projectId: string,
  query?: Record<string, string>,
  organizationId?: string,
): string {
  const q = new URLSearchParams(query).toString();
  const prefix = organizationId ? `/${organizationId}/${projectId}` : `/${projectId}`;
  return `${prefix}/connectors${q ? `?${q}` : ""}`;
}
