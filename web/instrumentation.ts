
export async function onRequestError(
  err: unknown,
  request: { path: string; method: string },
  context: { routerKind: string; routePath: string; renderSource?: string },
) {
  if (process.env.NEXT_RUNTIME !== "nodejs") return;
  const { logger } = await import("./lib/logger");
  // The route, never the path: a path spells an organization and a project by
  // the slugs of their names, and carries an invitation's token or a
  // callback's code.
  logger.error(
    {
      err,
      digest: (err as { digest?: string } | null)?.digest,
      method: request.method,
      routerKind: context.routerKind,
      routePath: context.routePath,
      renderSource: context.renderSource,
    },
    "unhandled request error",
  );
}

export async function register() {
  if (process.env.NEXT_RUNTIME !== "nodejs") return;

  const { logger } = await import("./lib/logger");
  const { validateEnv } = await import("./lib/env");

  if (process.env.NODE_ENV === "production") {
    validateEnv();
    logger.info("environment validated");
  } else {
    try {
      validateEnv();
    } catch (err) {
      logger.warn({ err }, "missing env vars — some features disabled");
    }
  }
}
