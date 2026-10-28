import type { NextRequest } from "next/server";

import { startSignIn } from "@/lib/auth/start";

export function GET(request: NextRequest) {
  return startSignIn(request);
}
