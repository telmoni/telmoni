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

// The panels are the create flows' own concern (`create-panels.test.tsx`);
// here a menu only has to open one, and whether the caller may create is set
// per test.
let canCreate = true;
vi.mock("@/components/create-project", () => ({
  useCanCreateProjects: () => canCreate,
}));
const openCreate = vi.fn();
vi.mock("@/components/console-ui-context", () => ({
  useConsoleUi: () => ({ openCreate }),
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

function renderSelector(
  badge?: React.ReactNode,
  projects: Project[] = [project("p1", "Atlas"), project("p2", "Beacon")],
) {
  return render(
    <StoreProvider
      user={null}
      organizations={[organization("acme", "owner"), organization("globex", "member")]}
      incomingInvites={[]}
      projectOffers={[]}
      activeOrganizationId="org_acme"
      flags={{ [Flag.Signup]: true }}
      projects={projects}
      projectsElsewhere={[elsewhere]}
    >
      <ResourceSelector organizationBadge={badge} />
    </StoreProvider>,
  );
}

function open(trigger: HTMLElement): HTMLElement {
  fireEvent.keyDown(trigger, { key: "Enter" });
  return screen.getByRole("menu");
}

beforeEach(() => {
  pathname = "/acme";
  canCreate = true;
  openCreate.mockClear();
  window.localStorage.clear();
});

describe("the switcher", () => {
  // A project is a click away from the organization's own pages, not a trip
  // to Projects: the slash is there, with a prompt where a name would be —
  // muted, so it never reads as a project called that.
  it("asks for a project after the organization at the organization level", () => {
    const { container } = renderSelector();
    expect(screen.getByRole("button", { name: "Organization: acme" })).toBeInTheDocument();
    const prompt = screen.getByRole("button", { name: "Select a project" });
    expect(within(prompt).getByTestId("project-prompt")).toHaveTextContent("Select a project");
    expect(within(prompt).getByTestId("project-prompt")).toHaveClass("text-muted-foreground");
    expect(container).toHaveTextContent("/");
  });

  it("lists the projects to pick at the organization level, none of them checked", () => {
    renderSelector();
    const menu = open(screen.getByRole("button", { name: "Select a project" }));
    expect(within(menu).getByRole("menuitem", { name: "Atlas" })).toHaveAttribute("href", "/acme/atlas");
    expect(within(menu).getByRole("menuitem", { name: "Atlas" })).not.toHaveAttribute("aria-current");
    expect(within(menu).getByRole("menuitem", { name: "Beacon" })).not.toHaveAttribute("aria-current");
    expect(within(menu).getByRole("menuitem", { name: "New project" })).toBeInTheDocument();
  });

  it("says there are no projects yet, and offers a new one", () => {
    renderSelector(undefined, []);
    const menu = open(screen.getByRole("button", { name: "Select a project" }));
    expect(menu).toHaveTextContent("No projects yet");
    expect(within(menu).getByRole("menuitem", { name: "New project" })).toBeInTheDocument();
  });

  it("names the project after a slash inside one", () => {
    pathname = "/acme/atlas";
    const { container } = renderSelector();
    expect(screen.getByRole("button", { name: "Project: Atlas" })).toBeInTheDocument();
    expect(container).toHaveTextContent("/");
  });

  it("draws the badge it is handed beside the organization's name", () => {
    renderSelector(<span data-testid="plan">Hobby</span>);
    const trigger = screen.getByRole("button", { name: /^Organization: acme/ });
    expect(within(trigger).getByTestId("plan")).toHaveTextContent("Hobby");
  });

  // The row a panel is opened from is gone once the menu shuts, so the panel
  // hands the focus back to the menu's trigger.
  it("opens New organization in its panel, handing it the menu's trigger", () => {
    renderSelector();
    const trigger = screen.getByRole("button", { name: "Organization: acme" });
    fireEvent.click(within(open(trigger)).getByRole("menuitem", { name: "New organization" }));
    expect(openCreate).toHaveBeenCalledWith("organization", trigger);
  });

  it("opens New project in its panel, handing it the menu's trigger", () => {
    renderSelector();
    const trigger = screen.getByRole("button", { name: "Select a project" });
    fireEvent.click(within(open(trigger)).getByRole("menuitem", { name: "New project" }));
    expect(openCreate).toHaveBeenCalledWith("project", trigger);
  });

  it("lists every organization the person can stand in, the active one first, with its settings", () => {
    renderSelector();
    const menu = open(screen.getByRole("button", { name: "Organization: acme" }));
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
    const menu = open(screen.getByRole("button", { name: "Project: Atlas" }));
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
    canCreate = false;
    renderSelector();
    const menu = open(screen.getByRole("button", { name: "Project: Atlas" }));
    expect(within(menu).queryByRole("menuitem", { name: "New project" })).toBeNull();
  });
});
