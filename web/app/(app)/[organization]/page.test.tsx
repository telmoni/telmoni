import { describe, expect, it, vi } from "vitest";

const redirect = vi.hoisted(() =>
  vi.fn(() => {
    throw new Error("redirected");
  }),
);
vi.mock("next/navigation", () => ({ redirect }));

import OrganizationPage from "./page";

describe("OrganizationPage", () => {
  it("opens the organization on its projects", async () => {
    await expect(
      OrganizationPage({ params: Promise.resolve({ organization: "acme" }) } as PageProps<"/[organization]">),
    ).rejects.toThrow("redirected");
    expect(redirect).toHaveBeenCalledWith("/acme/projects");
  });
});
