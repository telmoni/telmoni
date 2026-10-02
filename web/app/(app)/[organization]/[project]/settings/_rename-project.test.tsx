// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const refresh = vi.hoisted(() => vi.fn());
const replace = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh, replace }),
}));

vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));

const renameProject = vi.hoisted(() => vi.fn());
vi.mock("@/lib/store", () => ({ useRenameProject: () => renameProject }));

const updateProjectNameAction = vi.hoisted(() =>
  vi.fn<
    (projectId: string, name: string) => Promise<{ error: string | null; movedTo?: string }>
  >(async () => ({ error: null })),
);
vi.mock("./actions", () => ({ updateProjectNameAction }));

import { RenameProjectForm } from "./_rename-project";

beforeEach(() => {
  vi.clearAllMocks();
  updateProjectNameAction.mockResolvedValue({ error: null });
});

function form(initialName: string) {
  return (
    <RenameProjectForm
      projectId="project_1"
      organization="acme"
      slug="web"
      initialName={initialName}
      canEdit
    />
  );
}

async function save(name: string) {
  fireEvent.change(input(), { target: { value: name } });
  await act(async () => {
    fireEvent.submit(input().closest("form")!);
  });
}

function input(): HTMLInputElement {
  return screen.getByLabelText("Project name") as HTMLInputElement;
}

describe("RenameProjectForm", () => {
  it("follows a rename that happened elsewhere", () => {
    const { rerender } = render(form("Old"));
    expect(input().value).toBe("Old");

    // What a router.refresh() after someone else renames the project looks like.
    rerender(form("New"));

    expect(input().value).toBe("New");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("keeps what is being typed when the server value has not moved", () => {
    const { rerender } = render(form("Old"));
    fireEvent.change(input(), { target: { value: "Half-typed" } });

    // A refresh driven by something unrelated — the name itself is unchanged.
    rerender(form("Old"));

    expect(input().value).toBe("Half-typed");
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  // ⚠ A project's slug follows its name, so a rename moves the page out from
  // under the path it was made on. Staying put is a 404 on the next refresh.
  it("follows the project to the slug a rename moved it to", async () => {
    updateProjectNameAction.mockResolvedValue({ error: null, movedTo: "marketing-site" });
    render(form("Web"));

    await save("Marketing Site");

    expect(updateProjectNameAction).toHaveBeenCalledWith("project_1", "Marketing Site");
    expect(renameProject).toHaveBeenCalledWith("project_1", "Marketing Site", "marketing-site");
    expect(replace).toHaveBeenCalledWith("/acme/marketing-site/settings");
  });

  it("stays where it is when the slug did not move", async () => {
    render(form("Web"));

    await save("WEB");

    expect(renameProject).toHaveBeenCalledWith("project_1", "WEB", "web");
    expect(replace).not.toHaveBeenCalled();
    expect(refresh).toHaveBeenCalled();
  });

  it("moves nothing when the rename is refused", async () => {
    updateProjectNameAction.mockResolvedValue({ error: "a project named that already exists" });
    render(form("Web"));

    await save("Taken");

    expect(renameProject).not.toHaveBeenCalled();
    expect(replace).not.toHaveBeenCalled();
    expect(screen.getByText("a project named that already exists")).toBeInTheDocument();
  });
});
