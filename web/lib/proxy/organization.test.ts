import { sealData } from "iron-session";
import { NextRequest, NextResponse } from "next/server";
import { beforeEach, describe, expect, it } from "vitest";

import { proxy } from "@/proxy";

import { activeOrganizationCookie, namedOrganization, unescapedPath } from "./organization";

describe("namedOrganization", () => {
  it("reads the organization off the path's first segment", () => {
    expect(namedOrganization("/acme")).toBe("acme");
    expect(namedOrganization("/acme/web/api-keys")).toBe("acme");
    expect(namedOrganization("/acme/~/settings")).toBe("acme");
  });

  // Account, `/console` and every route handler: the cookie stands in.
  it.each(["/", "/account/settings", "/console", "/api/events", "/connect/slack/start", "/plans"])(
    "names none on %s",
    (pathname) => {
      expect(namedOrganization(pathname)).toBeNull();
    },
  );

  // An id is redirected to its slug by the layout; until then the request
  // acts wherever the cookie points, and nothing is drawn for it.
  it("names none for an id, or a segment no slug can be", () => {
    expect(namedOrganization("/org_7bQx2mNv9BcK4dLp/~/billing")).toBeNull();
    expect(namedOrganization("/Acme/web")).toBeNull();
    expect(namedOrganization("/~/settings")).toBeNull();
  });
});

describe("unescapedPath", () => {
  // `~` needs no escaping, and a mail client may escape it anyway.
  it("spells an escaped `~` plainly, where an organization's pages sit", () => {
    expect(unescapedPath("/acme/%7E/settings")).toBe("/acme/~/settings");
    expect(unescapedPath("/acme/%7e/billing")).toBe("/acme/~/billing");
    expect(unescapedPath("/org_7bQx2mNv9BcK4dLp/%7E/audit-log")).toBe(
      "/org_7bQx2mNv9BcK4dLp/~/audit-log",
    );
    expect(unescapedPath("/acme/%7E")).toBe("/acme/~");
  });

  it("leaves every other path as it came", () => {
    for (const pathname of [
      "/acme/~/settings",
      "/acme/web",
      "/%7E/settings",
      "/acme/web/%7E",
      "/acme/%7Ex/settings",
      "/api/events",
    ]) {
      expect(unescapedPath(pathname), pathname).toBeNull();
    }
  });
});

describe("activeOrganizationCookie", () => {
  // What `document.cookie` is handed. Readable by script on purpose: the
  // browser writes it, and it claims nothing. The organization's id, which a
  // rename leaves where it is.
  it("spells the cookie for the whole site, for thirty days", () => {
    expect(activeOrganizationCookie("org_7bQx2mNv9BcK4dLp", false)).toBe(
      "telmoni-organization=org_7bQx2mNv9BcK4dLp; Path=/; Max-Age=2592000; SameSite=Lax",
    );
  });

  it("is Secure where the page is", () => {
    expect(activeOrganizationCookie("org_7bQx2mNv9BcK4dLp", true)).toBe(
      "telmoni-organization=org_7bQx2mNv9BcK4dLp; Path=/; Max-Age=2592000; SameSite=Lax; Secure",
    );
    expect(activeOrganizationCookie("org_7bQx2mNv9BcK4dLp", true)).not.toMatch(/httponly/i);
  });
});

const SECRET = "test-secret-that-is-32-chars-long!!";
const COOKIE = "telmoni-organization";
// How `NextResponse.next({ request })` carries the headers a proxy forwards:
// the names it keeps, and each one's value.
const FORWARDED = "x-middleware-override-headers";
const forwarded = (name: string) => `x-middleware-request-${name}`;

async function run(request: NextRequest): Promise<NextResponse> {
  const res = await proxy(request, {} as never);
  if (!(res instanceof NextResponse)) throw new Error("the proxy answered no NextResponse");
  return res;
}

/** A signed-in request for `path`. */
async function visit(path: string, init: { cookie?: string; headers?: Record<string, string> } = {}) {
  const session = await sealData({ userId: "user_1" }, { password: SECRET });
  const cookies = [`telmoni_session=${session}`, ...(init.cookie ? [init.cookie] : [])];
  return run(
    new NextRequest(`http://localhost:3000${path}`, {
      headers: { ...init.headers, cookie: cookies.join("; ") },
    }),
  );
}

describe("the proxy and the organization a request acts in", () => {
  beforeEach(() => {
    process.env.AUTH_SECRET = SECRET;
  });

  it("hands the server the organization the path names", async () => {
    const res = await visit("/acme/web/api-keys");
    expect(res.headers.get(forwarded("x-telmoni-organization"))).toBe("acme");
  });

  it("hands on the path and query, for a layout to redirect from", async () => {
    const res = await visit("/org_1/project_1/connectors?connected=slack");
    expect(res.headers.get(forwarded("x-telmoni-path"))).toBe(
      "/org_1/project_1/connectors?connected=slack",
    );
  });

  // ⚠ Both headers are the proxy's to set. A client's own copy must not reach
  // the server as if the path had named an organization, nor as the path a
  // layout redirects from.
  it("drops a client's own copy of the organization header", async () => {
    const res = await visit("/account/settings", {
      headers: { "x-telmoni-organization": "globex" },
    });
    expect(res.headers.get(forwarded("x-telmoni-organization"))).toBeNull();
    expect(res.headers.get(FORWARDED)?.split(",")).not.toContain("x-telmoni-organization");
  });

  it("replaces a client's copy with what the path says", async () => {
    const res = await visit("/acme/web", {
      headers: { "x-telmoni-organization": "globex", "x-telmoni-path": "//evil.example" },
    });
    expect(res.headers.get(forwarded("x-telmoni-organization"))).toBe("acme");
    expect(res.headers.get(forwarded("x-telmoni-path"))).toBe("/acme/web");
  });

  // ⚠ The proxy cannot tell a page that opens from a prefetch: Next strips
  // the headers that mark one before it runs. The router prefetches every
  // link it draws, and the resource selector's point into every organization,
  // so a cookie written here would follow a link nobody followed. The
  // console writes it in the browser instead.
  it("never writes the cookie that remembers an organization", async () => {
    for (const path of ["/acme/web", "/globex/~/members", "/account/settings"]) {
      const res = await visit(path, { cookie: `${COOKIE}=acme` });
      expect(res.cookies.get(COOKIE), path).toBeUndefined();
    }
    expect((await visit("/acme/web")).cookies.get(COOKIE)).toBeUndefined();
  });

  // Ahead of the sign-in gate, so the path a visitor comes back to is the
  // one the router can match.
  it("redirects an escaped `~` to the plain one, query kept", async () => {
    const res = await run(
      new NextRequest("http://localhost:3000/org_1/%7E/billing?status=success"),
    );
    expect(res.status).toBe(308);
    expect(res.headers.get("location")).toBe(
      "http://localhost:3000/org_1/~/billing?status=success",
    );
  });

  it("sends a signed-out visitor to sign in, with the organization's path to come back to", async () => {
    const res = await run(new NextRequest("http://localhost:3000/acme/~/settings"));
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(
      "http://localhost:3000/auth/login?returnTo=%2Facme%2F%7E%2Fsettings",
    );
  });
});
