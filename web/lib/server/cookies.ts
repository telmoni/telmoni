import { cookies } from "next/headers";

export const ACTIVE_ORGANIZATION_COOKIE = "telmoni-active-organization";

/// Point the console at `organizationId` for the next 30 days.
///
/// ⚠ Writing this cookie CLAIMS nothing. `getServerContext` sends it to auth's
/// `/me` as the organization to act in, and auth honours it only when the
/// person is in that organization — so a caller cannot widen their reach by
/// setting it. Every writer still owes its own check, because a cookie
/// silently ignored on read is a switch that looks like it worked.
export async function setActiveOrganizationCookie(organizationId: string) {
  const jar = await cookies();
  jar.set(ACTIVE_ORGANIZATION_COOKIE, organizationId, {
    path: "/",
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
    sameSite: "lax",
    maxAge: 60 * 60 * 24 * 30,
  });
}
