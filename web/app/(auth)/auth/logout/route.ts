import { type NextRequest, NextResponse } from "next/server";

import { destroySession } from "@/lib/auth/session";
import { env } from "@/lib/env";

async function performLogout(): Promise<NextResponse> {
  const providerLogout = await destroySession({ redirectThroughProvider: true });
  return NextResponse.redirect(
    providerLogout ?? new URL("/", env.AUTH_URL),
    303,
  );
}

export function POST() {
  return performLogout();
}

export function GET(request: NextRequest) {
  if (request.headers.get("sec-fetch-site") === "cross-site") {
    return NextResponse.redirect(new URL("/", env.AUTH_URL));
  }
  return performLogout();
}
