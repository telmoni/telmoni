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
let ceiling: ProjectCeiling | null = null;
const addProject = vi.fn();
vi.mock("@/lib/store", () => ({
  useActiveOrganization: () => ({ organizationId: "org_acme", slug: "acme", name: "Acme", role }),
  useAddProject: () => addProject,
  useProjectCeiling: () => ceiling,
  useUser: () => ({ id: "u1", email: "owner@acme.test" }),
}));

const requestContentModeCodeAction = vi.fn();
const updateContentModeAction = vi.fn();
vi.mock("@/app/(app)/[organization]/[project]/settings/actions", () => ({
  requestContentModeCodeAction: (...args: unknown[]) => requestContentModeCodeAction(...args),
  updateContentModeAction: (...args: unknown[]) => updateContentModeAction(...args),
}));

const createProjectAction = vi.fn();
const createOrganizationAction = vi.fn();
vi.mock("@/app/(app)/actions", () => ({
  createProjectAction: (...args: unknown[]) => createProjectAction(...args),
  createOrganizationAction: (...args: unknown[]) => createOrganizationAction(...args),
}));

import type { Creating } from "@/components/console-ui-context";
import { CreatePanels } from "@/components/create-panels";
import type { ProjectCeiling } from "@/lib/server/entities/projects";

function panels(notice?: React.ReactNode, projectNotice?: React.ReactNode) {
  return render(<CreatePanels host="console.test" notice={notice} projectNotice={projectNotice} />);
}

const AT_CEILING = { limit: 2, held: 2, reached: true, detail: "Hobby holds 2 projects." };

beforeEach(() => {
  vi.clearAllMocks();
  role = "owner";
  ceiling = null;
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
      ok: true,
      project: { id: "p1", slug: "support-agent", name: "Support agent", role: "owner" },
    });
    panels();
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: " Support agent " } });
    fireEvent.click(screen.getByRole("button", { name: "Create project" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/acme/support-agent"));
    expect(createProjectAction).toHaveBeenCalledWith("Support agent", "org_acme", {});
    expect(addProject).toHaveBeenCalledTimes(1);
    expect(closeCreate).toHaveBeenCalledTimes(1);
  });

  it("starts with content off, and makes the project keep it once the owner's mailed code is typed", async () => {
    createProjectAction.mockResolvedValue({
      ok: true,
      project: { id: "p1", slug: "support-agent", name: "Support agent", role: "owner" },
    });
    requestContentModeCodeAction.mockResolvedValue({ ok: true });
    updateContentModeAction.mockResolvedValue({ ok: true });
    panels();
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
    fireEvent.click(screen.getByRole("radio", { name: "On" }));
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "Support agent" } });
    fireEvent.click(screen.getByRole("button", { name: "Create project" }));
    await waitFor(() => expect(requestContentModeCodeAction).toHaveBeenCalledWith("p1"));
    expect(push).not.toHaveBeenCalled();
    const code = await screen.findByLabelText(/sent to/);
    fireEvent.change(code, { target: { value: "123456" } });
    fireEvent.click(screen.getByRole("button", { name: "Keep content" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/acme/support-agent"));
    expect(updateContentModeAction).toHaveBeenCalledWith("p1", "on", { code: "123456" });
  });

  it("offers an admin no project that keeps content", () => {
    role = "admin";
    panels();
    expect(screen.getByRole("radio", { name: "On" })).toBeDisabled();
    expect(screen.getByRole("dialog", { name: "New project" })).toHaveTextContent(
      "Only the organization's owner can make a project that keeps content.",
    );
  });

  it("says what refused it and stays open", async () => {
    createProjectAction.mockResolvedValue({ ok: false, error: "A project with that name already exists." });
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

  // `components/extension/create-project-notice.tsx`: what lifts the ceiling.
  it("at the ceiling, opens with the notice, content off, and hands its fields on", async () => {
    ceiling = { ...AT_CEILING, liftable: true };
    createProjectAction.mockResolvedValue({ ok: true, href: "https://pay.example/session" });
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign });
    panels(undefined, <input type="hidden" name="plan" value="pro" data-testid="project-notice" />);
    expect(screen.getByTestId("project-notice")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "On" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "Web" } });
    fireEvent.click(screen.getByRole("button", { name: "Create project" }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("https://pay.example/session"));
    expect(createProjectAction).toHaveBeenCalledWith("Web", "org_acme", { plan: "pro" });
    expect(addProject).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("follows a path the action answers at the ceiling, shutting the panel", async () => {
    ceiling = { ...AT_CEILING, liftable: true };
    createProjectAction.mockResolvedValue({ ok: true, href: "/acme/billing/projects/o1" });
    panels(undefined, <span />);
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "Web" } });
    fireEvent.click(screen.getByRole("button", { name: "Create project" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/acme/billing/projects/o1"));
    expect(closeCreate).toHaveBeenCalledTimes(1);
  });

  it("where no plan holds more, says the ceiling and makes nothing", () => {
    ceiling = { ...AT_CEILING, liftable: false };
    panels(undefined, <span data-testid="project-notice" />);
    expect(screen.getByTestId("project-ceiling")).toHaveTextContent("Hobby holds 2 projects.");
    expect(screen.queryByTestId("project-notice")).toBeNull();
    fireEvent.change(screen.getByLabelText("Project name"), { target: { value: "Web" } });
    expect(screen.getByRole("button", { name: "Create project" })).toBeDisabled();
  });

  it("below the ceiling, says nothing of it", () => {
    ceiling = { ...AT_CEILING, held: 1, reached: false, liftable: true };
    panels(undefined, <span data-testid="project-notice" />);
    expect(screen.queryByTestId("project-notice")).toBeNull();
    expect(screen.queryByTestId("project-ceiling")).toBeNull();
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
    createOrganizationAction.mockResolvedValue({ ok: true, href: "/acme-robotics" });
    panels(<input type="hidden" name="plan" value="teams" />);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Acme Robotics" } });
    fireEvent.click(screen.getByRole("button", { name: "Create organization" }));
    await waitFor(() => expect(push).toHaveBeenCalledWith("/acme-robotics"));
    expect(createOrganizationAction).toHaveBeenCalledWith("Acme Robotics", "", { plan: "teams" });
    expect(closeCreate).toHaveBeenCalledTimes(1);
  });

  it("says a refusal of the URL under the URL", async () => {
    createOrganizationAction.mockResolvedValue({ ok: false, error: "That URL is taken.", field: "slug" });
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
