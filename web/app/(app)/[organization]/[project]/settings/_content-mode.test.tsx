// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ContentMode } from "@/lib/types/enums";

const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh }),
}));

const toast = vi.hoisted(() => ({ error: vi.fn(), success: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const updateContentModeAction = vi.hoisted(() =>
  vi.fn<(projectId: string, mode: ContentMode) => Promise<{ error: string | null }>>(
    async () => ({ error: null }),
  ),
);
vi.mock("./actions", () => ({ updateContentModeAction }));

import { ContentModeForm } from "./_content-mode";

beforeEach(() => {
  vi.clearAllMocks();
  updateContentModeAction.mockResolvedValue({ error: null });
});

function form({
  mode = ContentMode.Off,
  offered = [ContentMode.Off],
  canEdit = true,
}: { mode?: ContentMode; offered?: ContentMode[]; canEdit?: boolean } = {}) {
  return <ContentModeForm projectId="project_1" mode={mode} offered={offered} canEdit={canEdit} />;
}

function radio(name: string): HTMLElement {
  return screen.getByRole("radio", { name });
}

function save(): HTMLElement {
  return screen.getByRole("button", { name: "Save" });
}

/** Whether a button is held: marked so, and never `disabled`, which would
 *  drop the focus it has to the page. */
function isHeld(button: HTMLElement): boolean {
  expect(button).toBeEnabled();
  return button.getAttribute("aria-disabled") === "true";
}

describe("ContentModeForm", () => {
  it("is a group named for the mode, with the project's mode checked", () => {
    render(form());
    expect(screen.getByRole("radiogroup", { name: "Content mode" })).toBeInTheDocument();
    expect(radio("Off")).toHaveAttribute("aria-checked", "true");
    expect(radio("On")).toHaveAttribute("aria-checked", "false");
  });

  // ⚠ Until a table keeps content and our SDK seals it, `off` is the one mode
  // the server offers: the others are drawn, closed, and said to be.
  it("closes every mode the server does not offer, and says so", () => {
    render(form());
    expect(radio("Off")).toBeEnabled();
    expect(radio("On")).toBeDisabled();
    expect(radio("Sealed")).toBeDisabled();
    const note = screen.getByText("On and Sealed are not available yet.");
    expect(screen.getByRole("radiogroup")).toHaveAttribute("aria-describedby", note.id);
    expect(isHeld(save())).toBe(true);
  });

  it("sends nothing for a press on a held Save", async () => {
    render(form());
    await act(async () => {
      fireEvent.click(save());
    });
    expect(updateContentModeAction).not.toHaveBeenCalled();
  });

  it("names a single closed mode in the singular", () => {
    render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    expect(screen.getByText("Sealed is not available yet.")).toBeInTheDocument();
  });

  // The action revalidates the page, which sends it back with the new mode:
  // a refresh on top would draw it twice.
  it("saves once the owner picks another open mode, and draws nothing twice", async () => {
    render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("On"));
    expect(isHeld(save())).toBe(false);
    await act(async () => {
      fireEvent.click(save());
    });
    expect(updateContentModeAction).toHaveBeenCalledWith("project_1", ContentMode.On);
    expect(toast.success).toHaveBeenCalledWith("Content mode set to On.");
    expect(refresh).not.toHaveBeenCalled();
  });

  // A refusal can mean the page is out of date — the owner changed, or a
  // mode closed — so the page is drawn again, and the choice kept to retry.
  it("shows the server's refusal, keeps the choice, and draws the page again", async () => {
    updateContentModeAction.mockResolvedValue({
      error: "insufficient role: required owner, have admin",
    });
    render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("On"));
    await act(async () => {
      fireEvent.click(save());
    });
    expect(screen.getByText("insufficient role: required owner, have admin")).toBeInTheDocument();
    expect(radio("On")).toHaveAttribute("aria-checked", "true");
    expect(isHeld(save())).toBe(false);
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("holds every control while a save is on its way, and sends it once", async () => {
    // Settled before the test ends: a transition left pending would hold
    // React's action scope for every test after it.
    let answer: (result: { error: string | null }) => void = () => {};
    updateContentModeAction.mockImplementation(
      () =>
        new Promise<{ error: string | null }>((resolve) => {
          answer = resolve;
        }),
    );
    render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("On"));
    await act(async () => {
      fireEvent.click(save());
    });
    const saving = screen.getByRole("button", { name: "Saving..." });
    expect(isHeld(saving)).toBe(true);
    for (const name of ["Off", "On", "Sealed"]) expect(radio(name)).toBeDisabled();
    await act(async () => {
      fireEvent.click(saving);
    });
    expect(updateContentModeAction).toHaveBeenCalledTimes(1);

    await act(async () => {
      answer({ error: null });
    });
    expect(toast.success).toHaveBeenCalledWith("Content mode set to On.");
  });

  // A refusal because a mode closed brings the page back with it closed: the
  // choice falls back to the project's mode rather than stay on a closed one.
  it("drops a choice the server has stopped offering", () => {
    const { rerender } = render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("On"));
    expect(isHeld(save())).toBe(false);

    rerender(form({ offered: [ContentMode.Off] }));

    expect(radio("Off")).toHaveAttribute("aria-checked", "true");
    expect(radio("On")).toHaveAttribute("aria-checked", "false");
    expect(isHeld(save())).toBe(true);
  });

  // The server refuses anyone else; the form shows them the answer as text,
  // since a group of closed radios is dimmed and out of the tab order.
  it("shows anyone but the owner the mode as text, and who may change it", () => {
    render(form({ mode: ContentMode.Sealed, offered: [ContentMode.Off], canEdit: false }));
    expect(screen.getByText("Content mode:").parentElement).toHaveTextContent(
      "Content mode: Sealed",
    );
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(
      screen.getByText("Only the organization owner can change what this project keeps."),
    ).toBeInTheDocument();
  });

  it("follows a mode changed elsewhere, dropping an unsaved choice", () => {
    const offered = [ContentMode.Off, ContentMode.On, ContentMode.Sealed];
    const { rerender } = render(form({ offered }));
    fireEvent.click(radio("Sealed"));
    expect(isHeld(save())).toBe(false);

    // What a refresh after the owner changed it in another tab looks like.
    rerender(form({ mode: ContentMode.On, offered }));

    expect(radio("On")).toHaveAttribute("aria-checked", "true");
    expect(isHeld(save())).toBe(true);
  });
});
