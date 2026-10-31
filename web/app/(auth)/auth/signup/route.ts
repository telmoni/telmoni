import type { NextRequest } from "next/server";
import { z } from "zod";

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
  return z.email().safeParse(value).success ? value : undefined;
}
