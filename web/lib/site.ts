// The PRODUCT's constants: what every deployment of this console is. Who runs
// a deployment — the company, its support address, where its legal documents
// are — is not here; that is `lib/server/branding.ts`, read from the
// environment, so this repository names no operator.
export const REPO_URL: string | null = "https://github.com/telmoni/telmoni";
// The book is this console's own pages (`content/docs`, served by `app/docs`),
// so every link to it is a path on this origin.
export const DOCS_URL = "/docs";
export const PRODUCT_NAME = "Telmoni";
export const PRODUCT_DESCRIPTION =
  "Privacy-first, open-source monitoring for AI agents: alerts when a project's agents fail, stall, loop or overspend. Prompts stay in your process by default.";
// The security policy every repository shares, which also describes the hosted
// service's controls: it is published with the hosted service's legal pages,
// so every deployment links it on telmoni.com, where it exists.
export const SECURITY_POLICY_URL = "https://telmoni.com/docs/legal/security";
export const TWITTER_URL: string | null = null;
export const STATUS_URL: string | null = null;
export const DISCUSSIONS_URL: string | null = null;
export const SPONSORS_URL: string | null = null;
// The organization's support page, which every repository inherits: which
// channel is for what, and where to write about the hosted service.
export const SUPPORT_URL: string | null =
  "https://github.com/telmoni/.github/blob/main/SUPPORT.md";
