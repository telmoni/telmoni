// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { redirect, RedirectType } from "next/navigation";
import { Component, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";

const replace = vi.hoisted(() => vi.fn());
const refresh = vi.hoisted(() => vi.fn());
// Next's own redirect and `unstable_rethrow`; only the router is a stand-in.
vi.mock("next/navigation", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/navigation")>()),
  useRouter: () => ({ replace, refresh }),
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

const toastError = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({
  toast: { error: toastError, success: toastSuccess },
}));

const deleteProjectAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
vi.mock("./actions", () => ({ deleteProjectAction }));

import { DeleteProjectForm } from "./_delete-project";

beforeEach(() => {
  vi.clearAllMocks();
  deleteProjectAction.mockResolvedValue({ ok: true });
});

function open() {
  fireEvent.click(screen.getByRole("button", { name: "Delete project" }));
}

function confirmButton(): HTMLElement {
  const inDialog = screen
    .getAllByRole("button", { name: "Delete project" })
    .filter((b) => b.closest('[role="alertdialog"]'));
  expect(inDialog).toHaveLength(1);
  return inDialog[0]!;
}

describe("DeleteProjectForm", () => {
  it("holds the confirm shut until the project's name is typed", () => {
    render(<DeleteProjectForm projectId="project_1" projectName="Payments" />);
    open();

    expect(screen.getByText(/Type/)).toHaveTextContent("Payments");
    expect(confirmButton()).toBeDisabled();

    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Payment" },
    });
    expect(confirmButton(), "a prefix unlocked it").toBeDisabled();

    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Payments" },
    });
    expect(confirmButton()).toBeEnabled();
    expect(deleteProjectAction, "it deleted on a keystroke").not.toHaveBeenCalled();
  });

  // A deletion that landed leaves the page in the action itself: its redirect
  // rejects the call, and goes back to the router, never into a toast, with
  // nothing navigated or refreshed from here.
  it("hands a deletion's redirect to the router, claiming nothing itself", async () => {
    deleteProjectAction.mockImplementation(async () =>
      redirect("/console", RedirectType.replace),
    );
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <RouterStandIn>
        <DeleteProjectForm projectId="project_1" projectName="Payments" />
      </RouterStandIn>,
    );
    open();
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Payments" },
    });
    await act(async () => {
      fireEvent.click(confirmButton());
    });
    quiet.mockRestore();

    expect(deleteProjectAction).toHaveBeenCalledWith("project_1");
    expect(screen.getByTestId("redirected")).toHaveTextContent("NEXT_REDIRECT");
    expect(toastError).not.toHaveBeenCalled();
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(replace).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("stays put when the service refuses", async () => {
    deleteProjectAction.mockResolvedValue({
      ok: false,
      error: "only the organization owner can delete a project",
    });
    render(<DeleteProjectForm projectId="project_1" projectName="Payments" />);
    open();
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Payments" },
    });
    await act(async () => {
      fireEvent.click(confirmButton());
    });

    expect(toastError).toHaveBeenCalledWith(
      "only the organization owner can delete a project",
    );
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(replace).not.toHaveBeenCalled();
  });

  it("reports a network error without claiming anything happened", async () => {
    deleteProjectAction.mockRejectedValue(new Error("offline"));
    render(<DeleteProjectForm projectId="project_1" projectName="Payments" />);
    open();
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Payments" },
    });
    await act(async () => {
      fireEvent.click(confirmButton());
    });

    expect(toastError).toHaveBeenCalledWith("Network error. Try again.");
    expect(replace).not.toHaveBeenCalled();
  });

  it("names the project, and what goes with it, before it is opened", () => {
    render(<DeleteProjectForm projectId="project_1" projectName="Payments" />);
    const body = document.body.textContent ?? "";
    expect(body).toContain("Payments");
    expect(body).toMatch(/API keys/i);
    expect(body).toMatch(/cannot be undone/i);
  });
});
