import { NextResponse } from "next/server";

/** The console's own refusal, in the server's shape (RFC 9457), so a reader
 *  parses one; `about:blank` keeps it out of the book's catalogue. */
export function problem(
  status: number,
  title: string,
  init: { type?: string; detail?: string; headers?: Record<string, string> } = {},
): NextResponse {
  const { type = "about:blank", detail, headers } = init;
  return NextResponse.json(
    { type, title, status, ...(detail === undefined ? {} : { detail }) },
    { status, headers: { ...headers, "content-type": "application/problem+json" } },
  );
}

const PRIVATE = { "cache-control": "private, no-store" };

/** A read's answer: the session's own, so nothing in between keeps a copy. */
export function privateJson(body: unknown): NextResponse {
  return NextResponse.json(body, { headers: PRIVATE });
}

/** A read turned away, in the sentence the page shows; a 429 says when to try
 *  again. */
export function privateProblem(
  status: number,
  sentence: string,
  retryAfter?: number,
): NextResponse {
  const headers =
    retryAfter === undefined ? PRIVATE : { ...PRIVATE, "retry-after": String(retryAfter) };
  return problem(status, sentence, { headers });
}

/** Whether another site started the request. A browser names the site on
 *  every fetch; `none` is a load typed or bookmarked. */
export function crossSite(request: Request): boolean {
  const site = request.headers.get("sec-fetch-site");
  return !!site && site !== "same-origin" && site !== "none";
}
