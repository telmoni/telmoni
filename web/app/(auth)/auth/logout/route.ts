import { type NextRequest, NextResponse } from "next/server";

import { destroySession, getSession } from "@/lib/auth/session";
import { env } from "@/lib/env";

// ⚠ **A cross-site post is refused before anything else, because signing out
// DELETES THE COOKIES.** A page on another origin can auto-submit a form here;
// the browser withholds the session cookie (SameSite=Lax), the handler finds
// no session and still answers with a Set-Cookie that clears it, which the
// browser applies to the real one: a forced sign-out from any site, with no
// credential involved. Server Actions get an Origin check from Next for free;
// a Route Handler gets nothing, so this is that check, read two ways. Fetch
// Metadata names the site the post came from; `Origin`, which every browser
// sends on a cross-origin post whether or not it sends Fetch Metadata, names
// the page. Absent both, or `same-origin`/`none`, is the console's own form.
function crossSitePost(request: NextRequest): boolean {
  const site = request.headers.get("sec-fetch-site");
  if (site !== null && site !== "same-origin" && site !== "none") return true;
  const origin = request.headers.get("origin");
  return origin !== null && origin !== new URL(env.AUTH_URL).origin;
}

async function performLogout(): Promise<NextResponse> {
  const providerLogout = await destroySession({ redirectThroughProvider: true });
  return NextResponse.redirect(
    providerLogout ?? new URL("/", env.AUTH_URL),
    303,
  );
}

export function POST(request: NextRequest) {
  if (crossSitePost(request)) {
    return NextResponse.redirect(new URL("/", env.AUTH_URL), 303);
  }
  return performLogout();
}

// A navigation, not a post: a page's redirect of a dead session lands here
// and is let out, and another site's link is turned away. A live session is
// asked first (`confirm/page.tsx`), since a link is all it takes to arrive.
export async function GET(request: NextRequest) {
  if (request.headers.get("sec-fetch-site") === "cross-site") {
    return NextResponse.redirect(new URL("/", env.AUTH_URL));
  }
  if ((await getSession()) !== null) {
    return NextResponse.redirect(new URL("/auth/logout/confirm", env.AUTH_URL));
  }
  return performLogout();
}
