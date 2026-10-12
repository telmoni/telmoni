
import type { Instrumentation } from "next";

export const onRequestError: Instrumentation.onRequestError = async (err, request, context) => {
  if (process.env.NEXT_RUNTIME !== "nodejs") return;
  const { logger } = await import("./lib/logger");
  // The route, never the path: a path spells an organization and a project by
  // the slugs of their names, and carries an invitation's token or a
  // callback's code; nor the headers, which carry the session's cookie.
  // `routeType` tells a page's render from a Server Action's or a Route
  // Handler's on the same route.
  logger.error(
    {
      err,
      digest:
        typeof err === "object" && err !== null && "digest" in err ? String(err.digest) : undefined,
      method: request.method,
      routerKind: context.routerKind,
      routePath: context.routePath,
      routeType: context.routeType,
      renderSource: context.renderSource,
      revalidateReason: context.revalidateReason,
    },
    "unhandled request error",
  );
};

export async function register() {
  if (process.env.NEXT_RUNTIME !== "nodejs") return;

  const { logger } = await import("./lib/logger");
  const { env, validateEnv } = await import("./lib/env");

  if (process.env.NODE_ENV === "production") {
    validateEnv();
    logger.info("environment validated");
    if (env.TRUSTED_PROXY_HOPS === 0) {
      logger.warn(
        "TRUSTED_PROXY_HOPS=0: nothing in front of the console, so the per-address rate limits take the caller's word for its address",
      );
    }
  } else {
    try {
      validateEnv();
    } catch (err) {
      logger.warn({ err }, "missing env vars — some features disabled");
    }
  }
}
