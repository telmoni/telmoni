import { cookies } from "next/headers";

import {
  ACTIVE_ORGANIZATION_COOKIE,
  ACTIVE_ORGANIZATION_COOKIE_OPTIONS,
} from "@/lib/proxy/organization";

/// Point the paths that name no organization at this one for the next 30
/// days, for a lane that brings an organization back into reach without
/// opening it. Opening one is the console's own to record, in the browser
/// (`OrganizationSync`).
///
/// ⚠ Writing this cookie CLAIMS nothing. `getServerContext` sends it to auth's
/// `/me` as the organization to act in, and auth honours it only when the
/// person is in that organization — so a caller cannot widen their reach by
/// setting it.
export async function setActiveOrganizationCookie(organizationId: string) {
  const jar = await cookies();
  jar.set(ACTIVE_ORGANIZATION_COOKIE, organizationId, ACTIVE_ORGANIZATION_COOKIE_OPTIONS);
}
