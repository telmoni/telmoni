import { NextResponse, type NextRequest } from "next/server";

import { readBodyCapped } from "@/lib/api/body";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";

const MAX_BODY_BYTES = 1_048_576;

// Slack wants a 2xx within three seconds and retries on anything else, so the
// hop behind this has to give up well inside that: a slow service answers 503
// here and Slack tries again, where a 3.5s success would have counted as a
// failure anyway.
const UPSTREAM_TIMEOUT_MS = 2_500;

export async function POST(request: NextRequest): Promise<NextResponse> {
  const base = env.SERVER_URL;

  // The bytes, untouched: the signature is over them, and a re-serialized
  // body verifies against nothing. Read through the cap rather than
  // `arrayBuffer()` — see `readBodyCapped` for why the header alone is not one.
  //
  // A read that THROWS is a different answer from one that is too long: the
  // sender went away mid-upload, or the stream broke. Uncaught it became a 500
  // and an error-severity line for what is a routine disconnect, so it is a
  // 400 the vendor will simply retry.
  let raw: Uint8Array<ArrayBuffer> | null;
  try {
    raw = await readBodyCapped(request, MAX_BODY_BYTES);
  } catch {
    return NextResponse.json({ error: "could not read the body" }, { status: 400 });
  }
  if (raw === null) {
    return NextResponse.json({ error: "payload too large" }, { status: 413 });
  }

  const headers = new Headers();
  for (const name of ["x-slack-signature", "x-slack-request-timestamp", "x-slack-retry-num", "content-type"]) {
    const value = request.headers.get(name);
    if (value) headers.set(name, value);
  }
  headers.set("x-request-id", crypto.randomUUID());

  let upstream: Response;
  try {
    upstream = await fetchWithTimeout(
      `${base}/webhooks/slack`,
      { method: "POST", headers, body: raw, cache: "no-store" },
      UPSTREAM_TIMEOUT_MS,
    );
  } catch {
    return NextResponse.json({ error: "unavailable" }, { status: 503 });
  }

  const body = await upstream.text();
  const responseHeaders = new Headers();
  const upstreamType = upstream.headers.get("content-type");
  if (upstreamType) responseHeaders.set("content-type", upstreamType);
  responseHeaders.set("cache-control", "no-store, private");
  return new NextResponse(body, { status: upstream.status, headers: responseHeaders });
}
