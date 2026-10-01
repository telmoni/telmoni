// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh }),
}));

vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));

const renameProject = vi.hoisted(() => vi.fn());
vi.mock("@/lib/store", () => ({ useRenameProject: () => renameProject }));

const updateProjectNameAction = vi.hoisted(() =>
  vi.fn(async () => ({ error: null as string | null })),
);
vi.mock("./actions", () => ({ updateProjectNameAction }));

import { RenameProjectForm } from "./_rename-project";

beforeEach(() => {
  vi.clearAllMocks();
});

function input(): HTMLInputElement {
  return screen.getByLabelText("Project name") as HTMLInputElement;
}

describe("RenameProjectForm", () => {
  it("follows a rename that happened elsewhere", () => {
    const { rerender } = render(
      <RenameProjectForm projectId="project_1" initialName="Old" canEdit />,
    );
    expect(input().value).toBe("Old");

    // What a router.refresh() after someone else renames the project looks like.
    rerender(<RenameProjectForm projectId="project_1" initialName="New" canEdit />);

    expect(input().value).toBe("New");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("keeps what is being typed when the server value has not moved", () => {
    const { rerender } = render(
      <RenameProjectForm projectId="project_1" initialName="Old" canEdit />,
    );
    fireEvent.change(input(), { target: { value: "Half-typed" } });

    // A refresh driven by something unrelated — the name itself is unchanged.
    rerender(<RenameProjectForm projectId="project_1" initialName="Old" canEdit />);

    expect(input().value).toBe("Half-typed");
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });
});
