import type { NextRequest } from "next/server";

import { startSignIn } from "@/lib/auth/start";
import { MAX_EMAIL } from "@/lib/names";

export function GET(request: NextRequest) {
  return startSignIn(request, {
    signUp: true,
    loginHint: loginHint(request.nextUrl.searchParams.get("email")),
  });
}

function loginHint(raw: string | null): string | undefined {
  const value = raw?.trim() ?? "";
  if (value.length === 0 || [...value].length > MAX_EMAIL) return undefined;
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value) ? value : undefined;
}
