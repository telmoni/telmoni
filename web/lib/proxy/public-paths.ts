
import { EXTRA_PUBLIC_PREFIXES } from "@/lib/extension/public-paths";

const CORE_PUBLIC_PREFIXES: readonly string[] = [
  "/auth",
  // No page: the proxy redirects `/legal/*` to the deployment's published
  // documents (`LEGAL_URL`), and a deployment with none answers 404. Public
  // so the redirect happens before the sign-in gate.
  "/legal",
  "/invite",
  // The book (`app/docs`), read without a session.
  "/docs",
  "/api/health",
];

const PUBLIC_PREFIXES: readonly string[] = [
  ...CORE_PUBLIC_PREFIXES,
  ...EXTRA_PUBLIC_PREFIXES,
];

// The book's search, and the book as Markdown for a model (`lib/source.ts`).
const PUBLIC_FILES: readonly string[] = [
  "/robots.txt",
  "/sitemap.xml",
  "/api/search",
  "/llms.txt",
  "/llms-full.txt",
];

const TEST_SESSION_PATH = "/api/test/session";

// Nothing a machine calls is here: the public API, the CLI's lanes, ingest
// and the vendors' webhooks are the server's own, on the machine host, and
// the console relays none of them.
export function isPublic(pathname: string): boolean {
  if (pathname === "/") return true;
  if (PUBLIC_FILES.includes(pathname)) return true;
  if (
    pathname === TEST_SESSION_PATH &&
    process.env.ALLOW_TEST_SESSION === "true" &&
    process.env.NODE_ENV !== "production"
  ) {
    return true;
  }
  return PUBLIC_PREFIXES.some(
    (p) => pathname === p || pathname.startsWith(p + "/"),
  );
}
