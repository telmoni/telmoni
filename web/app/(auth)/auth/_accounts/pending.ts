import "server-only";

import { cookies } from "next/headers";
import { redirect } from "next/navigation";

import { PKCE_COOKIE, type PkceData, unsealPkce } from "@/lib/auth/session";

// The console's own sign-in pages are reached the way an external provider's
// are: `/auth/login` seals the CSRF state into the PKCE cookie and redirects
// here with the same state in the query, and the callback checks the two
// agree. A visit with no such cookie, or with another state, is not a sign-in
// in progress — a bookmark, a stale tab, a deployment whose login form is
// off — and goes back through the door, which decides where sign-in happens.
export async function pendingSignIn(state: string | undefined): Promise<PkceData> {
  const jar = await cookies();
  const sealed = jar.get(PKCE_COOKIE)?.value;
  const pkce = sealed ? await unsealPkce(sealed) : null;
  if (!state || !pkce || pkce.state !== state) redirect("/auth/login");
  return pkce;
}

// One value of a query parameter, or nothing: a repeated parameter is nothing.
export function one(value: string | string[] | undefined): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
