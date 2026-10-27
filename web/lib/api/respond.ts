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
