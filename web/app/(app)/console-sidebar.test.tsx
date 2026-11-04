// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

let pathname = "/settings/general";
vi.mock("next/navigation", () => ({ usePathname: () => pathname }));
vi.mock("@/lib/store", () => ({ useFlags: () => ({}) }));
vi.mock("@/lib/use-console-trail", () => ({
  useConsoleTrail: () => ["/acme/web/traces"],
  useLiveResources: () => ({ live: new Set(["project:acme/web"]), fallback: "/acme" }),
}));
vi.mock("@/lib/console-trail", () => ({ resolveReturnUrl: (trail: string[]) => trail[0] }));

import { ConsoleSidebar } from "@/components/console-sidebar";
import { SidebarProvider } from "@/components/sidebar-context";

function rail() {
  render(
    <SidebarProvider>
      <ConsoleSidebar version="0.0.0" />
    </SidebarProvider>,
  );
  return screen.getByRole("link", { name: /back to project/i });
}

describe("ConsoleSidebar: the way back", () => {
  beforeEach(() => {
    pathname = "/settings/general";
  });

  it("heads Settings' rail, leading to the page Settings was opened from", () => {
    const back = rail();
    expect(back).toHaveAttribute("href", "/acme/web/traces");
    expect(back.className).not.toMatch(/(^|\s)hidden(\s|$)/);
  });

  it("is hidden on a project's own rail, which needs none", () => {
    pathname = "/acme/web/traces";
    expect(rail().className).toMatch(/(^|\s)hidden(\s|$)/);
  });
});
