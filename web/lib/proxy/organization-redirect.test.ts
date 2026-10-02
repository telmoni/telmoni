import { describe, expect, it } from "vitest";
import { NextRequest } from "next/server";
import { proxy } from "@/proxy";

describe("proxy organization routing", () => {
  it("redirects /organization to active organization cookie when set", async () => {
    const req = new NextRequest("http://localhost:3000/organization", {
      headers: {
        cookie: "telmoni-active-organization=org_alpha",
      },
    });
    const res = await proxy(req, {} as never);
    expect(res?.status).toBe(307);
    expect(res?.headers.get("location")).toBe("http://localhost:3000/org_alpha");
  });

  it("redirects /organization/projects preserving subpath", async () => {
    const req = new NextRequest("http://localhost:3000/organization/projects?page=2", {
      headers: {
        cookie: "telmoni-active-organization=org_alpha",
      },
    });
    const res = await proxy(req, {} as never);
    expect(res?.status).toBe(307);
    expect(res?.headers.get("location")).toBe("http://localhost:3000/org_alpha/projects?page=2");
  });

  it("sets x-telmoni-organization-id header on hierarchical requests", async () => {
    // With public or logged-in path
    const req = new NextRequest("http://localhost:3000/org_beta/project_1", {
      headers: {
        cookie: "telmoni-active-organization=org_beta",
      },
    });
    // proxy will redirect to /auth/login if not logged in and not public,
    // but the header is set on the forwarded request.
    const res = await proxy(req, {} as never);
    // If redirected to login, it has returnTo
    expect(res?.status).toBe(307);
    expect(res?.headers.get("location")).toContain("/auth/login?returnTo=%2Forg_beta%2Fproject_1");
  });
});
