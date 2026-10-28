// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { redirect, RedirectType } from "next/navigation";
import { Component, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";

const refresh = vi.hoisted(() => vi.fn());
const replace = vi.hoisted(() => vi.fn());
// Next's own redirect and `unstable_rethrow`; only the router is a stand-in.
vi.mock("next/navigation", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/navigation")>()),
  useRouter: () => ({ refresh, replace }),
}));

// Where a redirect thrown out of the form lands, as Next's router's own
// boundary catches it in the console.
class RouterStandIn extends Component<{ children: ReactNode }, { error: unknown }> {
  state = { error: null as unknown };
  static getDerivedStateFromError(error: unknown) {
    return { error };
  }
  render() {
    const { error } = this.state;
    return error ? (
      <p data-testid="redirected">{(error as Error).message}</p>
    ) : (
      this.props.children
    );
  }
}

const toastSuccess = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: toastSuccess },
}));

const renameProject = vi.hoisted(() => vi.fn());
vi.mock("@/lib/store", () => ({ useRenameProject: () => renameProject }));

const updateProjectNameAction = vi.hoisted(() =>
  vi.fn<(projectId: string, name: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
vi.mock("./actions", () => ({ updateProjectNameAction }));

import { RenameProjectForm } from "./_rename-project";

beforeEach(() => {
  vi.clearAllMocks();
  updateProjectNameAction.mockResolvedValue({ ok: true });
});

function form(initialName: string) {
  return <RenameProjectForm projectId="project_1" slug="web" initialName={initialName} canEdit />;
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
  // under the path it was made on. The action takes it on to the new path
  // itself: its redirect rejects the call and goes back to the router, with
  // nothing navigated, renamed or claimed from here.
  it("hands a rename's move to the router, claiming nothing itself", async () => {
    updateProjectNameAction.mockImplementation(async () =>
      redirect("/acme/marketing-site/settings", RedirectType.replace),
    );
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<RouterStandIn>{form("Web")}</RouterStandIn>);

    await save("Marketing Site");
    quiet.mockRestore();

    expect(updateProjectNameAction).toHaveBeenCalledWith("project_1", "Marketing Site");
    expect(screen.getByTestId("redirected")).toHaveTextContent("NEXT_REDIRECT");
    expect(renameProject).not.toHaveBeenCalled();
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(replace).not.toHaveBeenCalled();
  });

  // The action's revalidation draws the page again with the new name: a
  // refresh here on top would draw it twice.
  it("stays where it is when the slug did not move", async () => {
    render(form("Web"));

    await save("WEB");

    expect(renameProject).toHaveBeenCalledWith("project_1", "WEB", "web");
    expect(replace).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("moves nothing when the rename is refused", async () => {
    updateProjectNameAction.mockResolvedValue({ ok: false, error: "a project named that already exists" });
    render(form("Web"));

    await save("Taken");

    expect(renameProject).not.toHaveBeenCalled();
    expect(replace).not.toHaveBeenCalled();
    expect(screen.getByText("a project named that already exists")).toBeInTheDocument();
  });
});
