
import { EXTRA_PUBLIC_PREFIXES } from "@/lib/extension/public-paths";

const CORE_PUBLIC_PREFIXES: readonly string[] = [
  "/auth",
  // No page: the proxy redirects `/legal/*` to the deployment's published
  // documents (`LEGAL_URL`), and a deployment with none answers 404. Public
  // so the redirect happens before the sign-in gate.
  "/legal",
  "/invite",
  "/api/health",
  "/v1",
  // The CLI's door. Bearer-authenticated by auth, never by a cookie, so the
  // proxy's session redirect has nothing to say to it.
  "/cli",
];

const PUBLIC_PREFIXES: readonly string[] = [
  ...CORE_PUBLIC_PREFIXES,
  ...EXTRA_PUBLIC_PREFIXES,
];

const PUBLIC_FILES: readonly string[] = ["/robots.txt", "/sitemap.xml"];

const TEST_SESSION_PATH = "/api/test/session";

// The vendor webhook relays. Each sends a signature and no session cookie,
// and the service behind it verifies the signature; without these the proxy
// answers 401 before the bytes ever reach a verifier. Exact paths, never a
// subtree.
const WEBHOOK_PATHS: readonly string[] = ["/api/webhooks/slack"];

export function isPublic(pathname: string): boolean {
  if (pathname === "/") return true;
  if (PUBLIC_FILES.includes(pathname)) return true;
  if (WEBHOOK_PATHS.includes(pathname)) return true;
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
