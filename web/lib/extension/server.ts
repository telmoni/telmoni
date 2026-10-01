// The console's server surface for a console built on this one: the session,
// the identity headers every server call carries, the fetch and rate-limit
// wrappers, and the server-rendered shell. See `./ui` for why this exists.
// `PaperShell` is here, not there, because its footer reads the environment.
import "server-only";

export { extractProblem, fetchWithTimeout, tryFetchWithTimeout } from "@/lib/api/fetch";
export { rateLimit, sessionKey } from "@/lib/api/rate-limit";
export type { SessionData } from "@/lib/auth/session";
export { env } from "@/lib/env";
export { Flag } from "@/lib/flags";
export { ownerContact } from "@/lib/identity";
export { logger } from "@/lib/logger";
export { branding } from "@/lib/server/branding";
export { activeOrganization, getServerContext } from "@/lib/server/data";
export {
  identityContext,
  organizationHeaders,
  sessionHeaders,
  type IdentityContext,
} from "@/lib/server/entities/identity-context";
export { featureOff } from "@/lib/server/flags";
export { SWITCHED_ORGANIZATION } from "@/lib/server/identity";
export { getServerSession } from "@/lib/server/session";
export { PaperShell } from "@/components/paper-shell";
