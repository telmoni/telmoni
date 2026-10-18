// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, replace: vi.fn(), refresh: vi.fn(), prefetch: vi.fn() }),
  unstable_rethrow: () => {},
}));

const uncover = vi.fn();
const cover = vi.fn(() => uncover);
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: document.body, cover }),
}));

let creating: Creating | null = null;
const closeCreate = vi.fn();
vi.mock("@/components/console-ui-context", () => ({
  useConsoleUi: () => ({ creating, closeCreate }),
}));

let role = "owner";
const addProject = vi.fn();
vi.mock("@/lib/store", () => ({
  useActiveOrganization: () => ({ organizationId: "org_acme", slug: "acme", name: "Acme", role }),
  useAddProject: () => addProject,
  useRoles: () => ({ organization: role }),
}));

const createProjectAction = vi.fn();
const createOrganizationAction = vi.fn();
vi.mock("@/app/(app)/actions", () => ({
  createProjectAction: (...args: unknown[]) => createProjectAction(...args),
  createOrganizationAction: (...args: unknown[]) => createOrganizationAction(...args),
}));

import type { Creating } from "@/components/console-ui-context";
import { CreatePanels } from "@/components/create-panels";

function panels(notice?: React.ReactNode) {
  return render(<CreatePanels host="console.test" notice={notice} />);
}

beforeEach(() => {
  vi.clearAllMocks();
  role = "owner";
  creating = null;
});

describe("CreatePanels", () => {
  it("draws nothing while none is open", () => {
    panels();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("New project", () => {
  beforeEach(() => {
    creating = { kind: "project", returnTo: null, opened: 1 };
  });

  it("opens over the page whole with its name field focused, naming the organization it lands in", () => {
    const { unmount } = panels();
    const panel = screen.getByRole("dialog", { name: "New project" });
    expect(panel).toHaveAttribute("aria-modal", "false");
    expect(panel).toHaveAttribute("data-mode", "full");
    expect(document.activeElement).toBe(within(panel).getByLabelText("Project name"));
    expect(panel).toHaveTextContent("OrganizationAcme");
    expect(cover).toHaveBeenCalledTimes(1);
    unmount();
    expect(uncover).toHaveBeenCalledTimes(1);
  });

  it("creates it in that organization and opens it", async () => {
    createProjectAction.mockResolvedValue({
      error: null,
      project: { id: "p1", slug: "support-agent", name: "Support agent", role: "owner" },
    });
    panels();
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: " Support agent " } });
    fireEvent.click(screen.getByRole("button", { name: "Create project" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/acme/support-agent"));
    expect(createProjectAction).toHaveBeenCalledWith("Support agent", "org_acme");
    expect(addProject).toHaveBeenCalledTimes(1);
    expect(closeCreate).toHaveBeenCalledTimes(1);
  });

  it("says what refused it and stays open", async () => {
    createProjectAction.mockResolvedValue({ error: "A project with that name already exists." });
    panels();
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "Web" } });
    fireEvent.click(screen.getByRole("button", { name: "Create project" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("A project with that name already exists.");
    expect(closeCreate).not.toHaveBeenCalled();
  });

  it("shuts on Escape from inside, Cancel and Close, handing the focus back to where it was opened", () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    creating = { kind: "project", returnTo: opener, opened: 1 };
    const { unmount } = panels();
    fireEvent.keyDown(screen.getByLabelText("Project name"), { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(closeCreate).toHaveBeenCalledTimes(3);
    unmount();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  // Asked for again over an open one — New project in the menu, or `c` —
  // it comes fresh and focused, as a dialog opened again did.
  it("comes fresh and focused when asked for again", () => {
    const { rerender } = panels();
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "Web" } });
    screen.getByRole("button", { name: "Cancel" }).focus();
    creating = { kind: "project", returnTo: null, opened: 2 };
    rerender(<CreatePanels host="console.test" />);
    const field = screen.getByLabelText("Project name");
    expect(field).toHaveValue("");
    expect(document.activeElement).toBe(field);
  });

  it("draws nothing for somebody who may create no project", () => {
    role = "member";
    panels();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("New organization", () => {
  beforeEach(() => {
    creating = { kind: "organization", returnTo: null, opened: 1 };
  });

  // `components/extension/create-organization-notice.tsx`: what a console
  // built on this one says a new organization starts on.
  it("opens with the notice it is handed, its name field focused, the host ahead of its URL", () => {
    panels(<p data-testid="notice">A new one starts on Pro.</p>);
    const panel = screen.getByRole("dialog", { name: "New organization" });
    expect(document.activeElement).toBe(within(panel).getByLabelText("Name"));
    expect(within(panel).getByTestId("notice")).toHaveTextContent("A new one starts on Pro.");
    expect(panel).toHaveTextContent("console.test/");
  });

  it("hands the action the notice's own fields by name, and opens what it answers", async () => {
    createOrganizationAction.mockResolvedValue({ error: null, href: "/acme-robotics" });
    panels(<input type="hidden" name="plan" value="teams" />);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Acme Robotics" } });
    fireEvent.click(screen.getByRole("button", { name: "Create organization" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/acme-robotics"));
    expect(createOrganizationAction).toHaveBeenCalledWith("Acme Robotics", "", { plan: "teams" });
    expect(closeCreate).toHaveBeenCalledTimes(1);
  });

  it("says a refusal of the URL under the URL", async () => {
    createOrganizationAction.mockResolvedValue({ error: "That URL is taken.", field: "slug" });
    panels();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Acme" } });
    fireEvent.change(screen.getByLabelText("URL"), { target: { value: "acme" } });
    fireEvent.click(screen.getByRole("button", { name: "Create organization" }));
    const said = await screen.findByRole("alert");
    expect(said).toHaveTextContent("That URL is taken.");
    expect(screen.getByLabelText("URL")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText("URL")).toHaveAttribute("aria-describedby", said.id);
    expect(closeCreate).not.toHaveBeenCalled();
  });
});
