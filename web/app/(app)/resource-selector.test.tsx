// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";

import { Flag } from "@/lib/flags";
import type { OrganizationEntry } from "@/lib/server/entities/organization";
import type { Project, ProjectEverywhere } from "@/lib/server/entities/projects";
import { StoreProvider } from "@/lib/store/provider";

import { ResourceSelector } from "@/components/resource-selector";

let pathname = "/acme";
vi.mock("next/navigation", () => ({
  usePathname: () => pathname,
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn(), prefetch: vi.fn() }),
}));

// The dialogs are the create flows' own concern; here they only have to be
// offered, so each is a stub, and the targets are set per test.
let targets: { id: string; label: string }[] = [];
vi.mock("@/components/create-project", () => ({
  CreateProjectDialog: () => null,
  useCreateProjectTargets: () => targets,
}));
vi.mock("@/components/create-organization", () => ({
  CreateOrganizationDialog: () => null,
}));

function organization(slug: string, role: OrganizationEntry["role"]): OrganizationEntry {
  return { organizationId: `org_${slug}`, slug, name: slug, role };
}

function project(id: string, name: string): Project {
  return { id, slug: name.toLowerCase(), name, role: "member" };
}

const elsewhere: ProjectEverywhere = {
  ...project("p3", "Comet"),
  organizationId: "org_initech",
  organizationSlug: "initech",
  organizationName: "Initech",
};

function renderSelector(badge?: React.ReactNode) {
  return render(
    <StoreProvider
      user={null}
      organizations={[organization("acme", "owner"), organization("globex", "member")]}
      incomingInvites={[]}
      projectOffers={[]}
      activeOrganizationId="org_acme"
      flags={{ [Flag.Signup]: true }}
      roles={{}}
      projects={[project("p1", "Atlas"), project("p2", "Beacon")]}
      projectsElsewhere={[elsewhere]}
    >
      <ResourceSelector host="console.test" organizationBadge={badge} />
    </StoreProvider>,
  );
}

function open(trigger: HTMLElement): HTMLElement {
  fireEvent.keyDown(trigger, { key: "Enter" });
  return screen.getByRole("menu");
}

beforeEach(() => {
  pathname = "/acme";
  targets = [{ id: "org_acme", label: "acme" }];
  window.localStorage.clear();
});

describe("the switcher", () => {
  it("names the organization alone at the organization level", () => {
    renderSelector();
    expect(screen.getByRole("button", { name: "Select an organization" })).toHaveTextContent("acme");
    expect(screen.queryByRole("button", { name: "Select a project" })).toBeNull();
  });

  it("names the project after a slash inside one", () => {
    pathname = "/acme/atlas";
    const { container } = renderSelector();
    expect(screen.getByRole("button", { name: "Select a project" })).toHaveTextContent("Atlas");
    expect(container).toHaveTextContent("/");
  });

  it("draws the badge it is handed beside the organization's name", () => {
    renderSelector(<span data-testid="plan">Hobby</span>);
    const trigger = screen.getByRole("button", { name: "Select an organization" });
    expect(within(trigger).getByTestId("plan")).toHaveTextContent("Hobby");
  });

  it("lists every organization the person can stand in, the active one first, with its settings", () => {
    renderSelector();
    const menu = open(screen.getByRole("button", { name: "Select an organization" }));
    const names = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent?.trim())
      .filter((text) => text && !text.endsWith("settings"));
    expect(names).toEqual(["acme", "globex", "Initech", "New organization"]);
    expect(within(menu).getByRole("menuitem", { name: "acme" })).toHaveAttribute("aria-current", "true");
    expect(within(menu).getByRole("menuitem", { name: "globex" })).toHaveAttribute("href", "/globex");
    expect(within(menu).getByRole("menuitem", { name: "acme settings" })).toHaveAttribute(
      "href",
      "/acme/settings",
    );
    // Reached through a project alone: nothing to open its settings with.
    expect(within(menu).queryByRole("menuitem", { name: "Initech settings" })).toBeNull();
  });

  it("lists the active organization's projects with their settings and a way to a new one", () => {
    pathname = "/acme/atlas";
    renderSelector();
    const menu = open(screen.getByRole("button", { name: "Select a project" }));
    expect(within(menu).getByRole("menuitem", { name: "Atlas" })).toHaveAttribute("aria-current", "true");
    expect(within(menu).getByRole("menuitem", { name: "Beacon" })).toHaveAttribute("href", "/acme/beacon");
    expect(within(menu).getByRole("menuitem", { name: "Beacon settings" })).toHaveAttribute(
      "href",
      "/acme/beacon/settings",
    );
    expect(within(menu).getByRole("menuitem", { name: "New project" })).toBeInTheDocument();
  });

  it("offers no new project to somebody who may create none", () => {
    pathname = "/acme/atlas";
    targets = [];
    renderSelector();
    const menu = open(screen.getByRole("button", { name: "Select a project" }));
    expect(within(menu).queryByRole("menuitem", { name: "New project" })).toBeNull();
  });
});
