// ⚠️ **`server-only` here guards every secret in the console at once.** The
// server's URL, the service secret and the session seal are read through
// this module, and a `"use client"` file that imports it — directly, or through
// anything that does — now fails the build instead of shipping the code that
// spends them to the browser. Next resolves the specifier itself; there is no
// package behind it.
import "server-only";

const REQUIRED = [
  "SERVER_URL",
  "SERVICE_SECRET",
  "AUTH_SECRET",
  "AUTH_URL",
  "REDIS_URL",
] as const;

type RequiredKey = (typeof REQUIRED)[number];

const AUTH_SECRET_MIN_BYTES = 32;

const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]", "::1"]);

class MissingEnvError extends Error {
  constructor(missing: string[]) {
    super(
      `Missing required env vars: ${missing.join(", ")}\nSee web/.env.local.example for documentation.`,
    );
    this.name = "MissingEnvError";
  }
}

class InvalidEnvError extends Error {
  constructor(key: string, reason: string) {
    super(`Invalid env var ${key}: ${reason}`);
    this.name = "InvalidEnvError";
  }
}

export function validateEnv(): void {
  const missing = REQUIRED.filter((k) => !process.env[k]);
  if (missing.length > 0) throw new MissingEnvError(missing);

  const authSecret = process.env.AUTH_SECRET!;
  const bytes = Buffer.byteLength(authSecret, "utf8");
  if (bytes < AUTH_SECRET_MIN_BYTES) {
    throw new InvalidEnvError(
      "AUTH_SECRET",
      `must be ≥${AUTH_SECRET_MIN_BYTES} bytes (got ${bytes}). ` +
        `Generate with: openssl rand -hex 32`,
    );
  }

  void env.AUTH_URL;
}

export function isSecureOrigin(value: string): boolean {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (url.protocol === "https:") return true;
  return url.protocol === "http:" && LOOPBACK_HOSTS.has(url.hostname);
}

function read(
  key: RequiredKey,
  opts: { stripTrailingSlash?: boolean } = {},
): string {
  const value = process.env[key];
  if (!value) throw new MissingEnvError([key]);
  return opts.stripTrailingSlash ? value.replace(/\/$/, "") : value;
}

export const env = {
  // The server: auth and notifications, one process, one origin. Every
  // service call the console makes goes here, with SERVICE_SECRET and the
  // session's bearer, and it is a HARD dependency — with it unreachable the
  // console serves nothing, because `/me` is the one door a session cannot
  // start without. The notification surfaces still degrade on their own:
  // their fetchers return null on a failed call and the page renders its
  // empty state.
  get SERVER_URL(): string {
    return read("SERVER_URL", { stripTrailingSlash: true });
  },

  get SERVICE_SECRET(): string {
    return read("SERVICE_SECRET");
  },

  get AUTH_SECRET(): string {
    return read("AUTH_SECRET");
  },

  get AUTH_URL(): string {
    const value = read("AUTH_URL", { stripTrailingSlash: true });
    if (process.env.NODE_ENV === "production" && !isSecureOrigin(value)) {
      throw new InvalidEnvError(
        "AUTH_URL",
        "must be an https:// origin in production (or an http:// loopback origin)",
      );
    }
    return value;
  },

  get REDIS_URL(): string {
    return read("REDIS_URL");
  },

  get REDIS_CA_CERT(): string | undefined {
    const v = process.env.REDIS_CA_CERT;
    return v && v.trim() ? v.trim() : undefined;
  },

  // Who runs this deployment: `lib/server/branding.ts` reads these. All
  // optional, documented here so the required-list check and this module
  // stay the one place the console's environment is described.
  get COMPANY_NAME(): string | undefined {
    const v = process.env.COMPANY_NAME;
    return v && v.trim() ? v.trim() : undefined;
  },
  get SUPPORT_EMAIL(): string | undefined {
    const v = process.env.SUPPORT_EMAIL;
    return v && v.trim() ? v.trim() : undefined;
  },
  get LEGAL_URL(): string | undefined {
    const v = process.env.LEGAL_URL;
    return v && v.trim() ? v.trim().replace(/\/$/, "") : undefined;
  },

  get ANALYTICS_INGEST_URL(): string | undefined {
    const v = process.env.ANALYTICS_INGEST_URL;
    return v && v.trim() ? v.trim() : undefined;
  },
  get ANALYTICS_WRITE_KEY(): string | undefined {
    const v = process.env.ANALYTICS_WRITE_KEY;
    return v && v.trim() ? v.trim() : undefined;
  },
} as const;
