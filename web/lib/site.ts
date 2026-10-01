// The PRODUCT's constants: what every deployment of this console is. Who runs
// a deployment — the company, its support address, where its legal documents
// are — is not here; that is `lib/server/branding.ts`, read from the
// environment, so this repository names no operator.
export const REPO_URL: string | null = null;
export const DOCS_URL = "https://docs.telmoni.com";
// The same host without the scheme, for anywhere that prints it rather than
// links it. Derived, because it was written out a second time in the search
// palette and the two spellings could disagree.
export const DOCS_HOST = DOCS_URL.replace(/^https?:\/\//, "");
export const PRODUCT_NAME = "Telmoni";
export const PRODUCT_DESCRIPTION =
  "Multi-tenant foundation: organizations and projects, members and roles, API tokens, and a hash-chained audit log.";
export const TWITTER_URL: string | null = null;
export const STATUS_URL: string | null = null;
export const DISCUSSIONS_URL: string | null = null;
export const SPONSORS_URL: string | null = null;
export const SITE_URL = (
  process.env.NEXT_PUBLIC_APP_URL || "https://example.com"
).replace(/\/$/, "");
