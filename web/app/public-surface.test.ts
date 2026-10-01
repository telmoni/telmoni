import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { isPublic } from "@/lib/proxy/public-paths";

const APP = __dirname;

// ⚠ **What our infrastructure is made of is not a fact an anonymous caller
// gets to collect.** The console once answered `/api/health` with the state of
// the Redis connection, which is worse than a leaked dependency name: it tells
// a stranger the exact moment the shared rate limiter fell back to a
// per-replica one, which is the moment to start spending it. The name went
// with it — there is no version of "external users can see which datastore we
// run" that helps them and several that help an attacker.
//
// The list is derived from `isPublic`, the same predicate the proxy uses to
// decide who gets in without a session, so a new public prefix pulls its
// routes in here without anyone remembering to.
//
// ⚠ **The claim survives; the product name does not.** The sign-in page and
// the CLI door may say what the product does — that is the mechanism a buyer
// of a multi-tenant chassis is entitled to check. Which engine implements it
// is not part of the claim, and naming it only tells somebody which CVEs and
// which default ports to try. The operator's own pages (about, security, the
// legal documents) are not in this repository at all.
//
// Known limit: this reads route files, not every module they import. It
// catches the mistake that actually happens — a handler or a page written to
// be helpful — not a determined leak buried three imports down.
const FORBIDDEN = [
  "redis",
  "ioredis",
  "memorystore",
  "memcache",
  "valkey",
  "postgres",
  "cloud sql",
  "cloudsql",
];

function routeFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      routeFiles(full, out);
    } else if (
      /^(route|page|layout|error|not-found)\.tsx?$/.test(entry.name) &&
      !entry.name.includes(".test.")
    ) {
      out.push(full);
    }
  }
  return out;
}

// `app/(auth)/auth/login/route.ts` is served at `/auth/login`: route groups
// are parentheses and do not appear in the URL, and the file name is not part
// of it either.
function routeOf(file: string): string {
  const segments = path
    .relative(APP, path.dirname(file))
    .split(path.sep)
    .filter((s) => s.length > 0 && !(s.startsWith("(") && s.endsWith(")")));
  return `/${segments.join("/")}`;
}

const PUBLIC_SOURCES = routeFiles(APP)
  .map((file) => ({ file: path.relative(APP, file), route: routeOf(file), full: file }))
  .filter(({ route }) => isPublic(route))
  // Comments are where a decision gets explained, and several of these
  // explain themselves by naming the thing they are protecting. Only what can
  // reach a response is in scope.
  .map((entry) => ({
    ...entry,
    code: readFileSync(entry.full, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, ""),
  }));

describe("the surface an unauthenticated caller can reach", () => {
  it("is not empty, or this whole file is asserting nothing", () => {
    expect(PUBLIC_SOURCES.length).toBeGreaterThan(3);
    expect(PUBLIC_SOURCES.map((s) => s.route)).toContain("/api/health");
  });

  it("names none of the things we run", () => {
    for (const { file, route, code } of PUBLIC_SOURCES) {
      for (const word of FORBIDDEN) {
        expect(
          code.toLowerCase(),
          `${file} (served at ${route}) names ${word} somewhere it could reach a response`,
        ).not.toContain(word);
      }
    }
  });
});
