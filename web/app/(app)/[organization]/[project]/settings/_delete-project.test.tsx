// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const replace = vi.hoisted(() => vi.fn());
const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace, refresh }),
}));

const toastError = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({
  toast: { error: toastError, success: toastSuccess },
}));

const deleteProjectAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
);
vi.mock("./actions", () => ({ deleteProjectAction }));

import { DeleteProjectForm } from "./_delete-project";

beforeEach(() => {
  vi.clearAllMocks();
  deleteProjectAction.mockResolvedValue({ error: null });
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

  it("deletes, then sends the person to /console without re-rendering the deleted project", async () => {
    render(<DeleteProjectForm projectId="project_1" projectName="Payments" />);
    open();
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "Payments" },
    });
    await act(async () => {
      fireEvent.click(confirmButton());
    });

    expect(deleteProjectAction).toHaveBeenCalledWith("project_1");
    expect(toastSuccess).toHaveBeenCalledWith("Payments deleted.");
    expect(replace).toHaveBeenCalledWith("/console");
    expect(refresh, "a refresh re-rendered the deleted project first").not.toHaveBeenCalled();
  });

  it("stays put when the service refuses", async () => {
    deleteProjectAction.mockResolvedValue({
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

    expect(toastError).toHaveBeenCalledWith("Network error. Please try again.");
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
