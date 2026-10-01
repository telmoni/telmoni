import { Problem } from "./problem";
import { logger } from "@/lib/logger";

const SERVICE_USER_AGENT = "telmoni-web/0.1.0";

// ⚠ **`ms` bounds the WHOLE exchange, body included — not the wait for
// headers.** The first version armed a timer and cleared it the moment `fetch`
// resolved, which is when the status line arrives; a service that answered
// 200 and then stalled mid-body held the caller's `await res.json()` for as
// long as undici's own body timeout allows, which is five minutes. Every
// console render awaits one of these inline, so that was a five-minute page.
// `AbortSignal.timeout` has no timer to clear: it fires at `ms` whatever state
// the response is in, and an abort after the headers rejects the body read.
//
// The caller's own `signal` is honoured alongside it. A Route Handler passes
// `request.signal` so a client that goes away cancels the upstream call
// instead of leaving it to run to completion for nobody.
//
// `cache: "no-store"` unless the caller says otherwise. Every URL this is
// given is a live service, and Next's default would fetch once at build time
// on any route it can prerender — baking one service's answer into the build
// for every request after. A caller that wants caching asks for it in words.
export async function fetchWithTimeout(
  url: string,
  options: RequestInit = {},
  ms = 10_000,
): Promise<Response> {
  const headers = new Headers(options.headers);
  if (!headers.has("user-agent")) {
    headers.set("user-agent", SERVICE_USER_AGENT);
  }
  const signals = [AbortSignal.timeout(ms)];
  if (options.signal) signals.push(options.signal);
  return fetch(url, {
    cache: "no-store",
    ...options,
    headers,
    signal: AbortSignal.any(signals),
  });
}

export async function tryFetchWithTimeout(
  url: string,
  options: RequestInit = {},
  ms = 10_000,
): Promise<Response | null> {
  try {
    return await fetchWithTimeout(url, options, ms);
  } catch (err) {
    logger.warn(
      { url, error: err instanceof Error ? err.message : String(err) },
      "write-path fetch failed",
    );
    return null;
  }
}

export function safeReturnTo(raw: string | undefined, base: URL): string {
  if (!raw) return "/";
  try {
    const url = new URL(raw, base);
    if (url.origin !== base.origin) return "/";
    const path = url.pathname + url.search;
    // ⚠ **The origin check above is not enough, because the path it leaves
    // behind gets re-resolved by the caller.** Every caller spends this value
    // as `new URL(returnTo, request.url)`, and `/..//evil.com` parses HERE as
    // same-origin — the `..` climbs to the root — while `url.pathname`
    // normalises to `//evil.com`. Handed back, that is protocol-relative, and
    // the caller's `new URL` reads it as `https://evil.com`. The whole
    // round-trip is same-origin at every step except the last.
    //
    // Reachable on `/auth/login`, `/auth/signup` and — sealed into the PKCE
    // cookie — on the post-sign-in redirect, which is the one a phisher wants:
    // it lands somebody on an attacker's page seconds after they authenticated.
    if (path.startsWith("//") || path.startsWith("/\\")) return "/";
    return path;
  } catch {
    return "/";
  }
}

export async function extractProblem(
  res: Response,
): Promise<{ message: string; problem: Problem | null }> {
  const ct = res.headers.get("content-type") ?? "";
  if (!ct.includes("json")) {
    const text = await res.text().catch(() => "");
    return {
      message: text.trim() || `request failed (${res.status})`,
      problem: null,
    };
  }
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    return { message: `request failed (${res.status})`, problem: null };
  }
  const parsed = Problem.safeParse(body);
  if (parsed.success) {
    const problem = parsed.data;
    const message =
      problem.detail && problem.detail.length > 0
        ? `${problem.title}: ${problem.detail}`
        : problem.title;
    return { message, problem };
  }
  if (body && typeof body === "object") {
    const b = body as Record<string, unknown>;
    if (typeof b.error === "string") {
      return { message: b.error, problem: null };
    }
  }
  return { message: `request failed (${res.status})`, problem: null };
}
