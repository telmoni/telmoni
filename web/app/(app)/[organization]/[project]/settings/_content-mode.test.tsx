// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ActionResult } from "@/lib/action-result";
import { ContentMode } from "@/lib/types/enums";

const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh }),
}));

const toast = vi.hoisted(() => ({ error: vi.fn(), success: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const updateContentModeAction = vi.hoisted(() =>
  vi.fn<
    (
      projectId: string,
      mode: ContentMode,
      options?: { code?: string; deleteContent?: boolean },
    ) => Promise<ActionResult>
  >(async () => ({ ok: true })),
);
const requestContentModeCodeAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<ActionResult>>(async () => ({ ok: true })),
);
const setDoorScrubAction = vi.hoisted(() => vi.fn(async () => ({ ok: true } as ActionResult)));
vi.mock("./actions", () => ({ updateContentModeAction, requestContentModeCodeAction, setDoorScrubAction }));

import { ContentModeForm } from "./_content-mode";
import { DoorScrubForm } from "./_retention";

describe("DoorScrubForm", () => {
  const policy = { door_scrub_secrets: true, door_scrub_pii: false };
  it("shows the project policy without permitting a member to change it", () => {
    render(<DoorScrubForm projectId="project_1" policy={policy} disabled />);
    expect(screen.getByRole("checkbox", { name: "Scrub secrets at the door" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Scrub personal data at the door" })).not.toBeChecked();
    expect(screen.getAllByRole("checkbox").every((control) => control.hasAttribute("disabled"))).toBe(true);
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  });
  it("submits both switches and surfaces a failed save", async () => {
    setDoorScrubAction.mockResolvedValueOnce({ ok: false, error: "Try again" });
    render(<DoorScrubForm projectId="project_1" policy={policy} disabled={false} />);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: "Scrub personal data at the door" }));
    await act(async () => { fireEvent.submit(screen.getByRole("button", { name: "Save" }).closest("form")!); });
    expect(setDoorScrubAction).toHaveBeenCalledWith("project_1", { door_scrub_secrets: true, door_scrub_pii: true });
    expect(toast.error).toHaveBeenCalledWith("Try again");
  });
});

beforeEach(() => {
  vi.clearAllMocks();
  vi.useRealTimers();
  updateContentModeAction.mockResolvedValue({ ok: true });
});

function form({
  mode = ContentMode.Off,
  offered = [ContentMode.Off],
  canEdit = true,
}: { mode?: ContentMode; offered?: ContentMode[]; canEdit?: boolean } = {}) {
  // Keyed as the page keys it: a mode or an offer changed draws a fresh form.
  return (
    <ContentModeForm
      key={`${mode}:${offered.join()}`}
      projectId="project_1"
      mode={mode}
      offered={offered}
      canEdit={canEdit}
      email="owner@example.com"
    />
  );
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

  // ⚠ Until a table keeps content, `off` is the one mode the server offers:
  // `on` is drawn, closed, and said to be.
  it("closes every mode the server does not offer, and says so", () => {
    render(form());
    expect(radio("Off")).toBeEnabled();
    expect(radio("On")).toBeDisabled();
    const note = screen.getByText("On is not available yet.");
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

  it("names closed modes together, in the plural", () => {
    render(form({ offered: [] }));
    expect(screen.getByText("Off and On are not available yet.")).toBeInTheDocument();
  });

  it("says nothing is closed when every mode is offered", () => {
    render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    expect(screen.queryByText(/not available yet/)).toBeNull();
    expect(screen.getByRole("radiogroup")).not.toHaveAttribute("aria-describedby");
  });

  // The action revalidates the page, which sends it back with the new mode:
  // a refresh on top would draw it twice.
  it("saves once the owner picks another open mode, and draws nothing twice", async () => {
    render(form({ mode: ContentMode.On, offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("Off"));
    expect(isHeld(save())).toBe(false);
    await act(async () => {
      fireEvent.click(save());
    });
    expect(updateContentModeAction).toHaveBeenCalledWith("project_1", ContentMode.Off, {
      deleteContent: false,
    });
    expect(toast.success).toHaveBeenCalledWith("Content mode set to Off.");
    expect(refresh).not.toHaveBeenCalled();
  });

  // ⚠ Leaving `off` is confirmed from the owner's inbox: the press mails the
  // code, and the switch spends the code typed.
  it("mails a code before leaving off, and switches with the code typed", async () => {
    render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("On"));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Send confirmation code" }));
    });
    expect(requestContentModeCodeAction).toHaveBeenCalledWith("project_1");
    expect(updateContentModeAction).not.toHaveBeenCalled();
    const field = screen.getByLabelText(/code sent to/);
    expect(screen.getByText("owner@example.com")).toBeInTheDocument();
    fireEvent.change(field, { target: { value: "12 34 56" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Keep content" }));
    });
    expect(updateContentModeAction).toHaveBeenCalledWith("project_1", ContentMode.On, {
      code: "123456",
    });
    expect(toast.success).toHaveBeenCalledWith("Content mode set to On.");
  });

  it("surfaces a refused resend and leaves the typed code available", async () => {
    requestContentModeCodeAction.mockResolvedValueOnce({ ok: true }).mockResolvedValueOnce({
      ok: false, error: "Please wait before requesting another code.",
    });
    vi.useFakeTimers({ shouldAdvanceTime: true });
    render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("On"));
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Send confirmation code" })); });
    const field = screen.getByLabelText(/code sent to/);
    fireEvent.change(field, { target: { value: "123456" } });
    expect(screen.getByRole("button", { name: "Resend code in 30s" })).toBeDisabled();
    act(() => { vi.advanceTimersByTime(30_000); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Resend code" })); });
    expect(screen.getByRole("alert")).toHaveTextContent("Please wait before requesting another code.");
    expect(field).toHaveValue("123456");
    expect(screen.getByRole("button", { name: "Resend code" })).toBeEnabled();
    expect(updateContentModeAction).not.toHaveBeenCalled();
  });

  // Back to `off` asks nothing, and offers to delete what the project kept,
  // saying what each choice leaves.
  it("offers to delete what the project kept as it goes off", async () => {
    render(form({ mode: ContentMode.On, offered: [ContentMode.Off, ContentMode.On] }));
    expect(screen.queryByRole("checkbox")).toBeNull();
    fireEvent.click(radio("Off"));
    expect(screen.getByText(/stays readable to its members/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Also delete what this project kept" }));
    expect(screen.getByText(/nothing reads it again; a backup taken before keeps a copy until it expires/)).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(save());
    });
    expect(updateContentModeAction).toHaveBeenCalledWith("project_1", ContentMode.Off, {
      deleteContent: true,
    });
    expect(requestContentModeCodeAction).not.toHaveBeenCalled();
    expect(toast.success).toHaveBeenCalledWith(
      "Content mode set to Off, and what the project kept is deleted.",
    );
  });

  // A refusal can mean the page is out of date — the owner changed, or a
  // mode closed — so the action draws the page again (its `refresh()`), and
  // the choice is kept to retry: a refresh here on top would draw it twice.
  it("shows the server's refusal, keeps the choice, and draws nothing twice", async () => {
    updateContentModeAction.mockResolvedValue({
      ok: false,
      error: "insufficient role: required owner, have admin",
    });
    render(form({ mode: ContentMode.On, offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("Off"));
    await act(async () => {
      fireEvent.click(save());
    });
    expect(screen.getByText("insufficient role: required owner, have admin")).toBeInTheDocument();
    expect(radio("Off")).toHaveAttribute("aria-checked", "true");
    expect(isHeld(save())).toBe(false);
    expect(refresh).not.toHaveBeenCalled();
  });

  it("holds every control while a save is on its way, and sends it once", async () => {
    // Settled before the test ends: a transition left pending would hold
    // React's action scope for every test after it.
    let answer: (result: ActionResult) => void = () => {};
    updateContentModeAction.mockImplementation(
      () =>
        new Promise<ActionResult>((resolve) => {
          answer = resolve;
        }),
    );
    render(form({ mode: ContentMode.On, offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("Off"));
    await act(async () => {
      fireEvent.click(save());
    });
    const saving = screen.getByRole("button", { name: "Saving..." });
    expect(isHeld(saving)).toBe(true);
    for (const name of ["Off", "On"]) expect(radio(name)).toBeDisabled();
    await act(async () => {
      fireEvent.click(saving);
    });
    expect(updateContentModeAction).toHaveBeenCalledTimes(1);

    await act(async () => {
      answer({ ok: true });
    });
    expect(toast.success).toHaveBeenCalledWith("Content mode set to Off.");
  });

  // A refusal because a mode closed brings the page back with it closed: the
  // choice falls back to the project's mode rather than stay on a closed one.
  it("drops a choice the server has stopped offering", () => {
    const { rerender } = render(form({ offered: [ContentMode.Off, ContentMode.On] }));
    fireEvent.click(radio("On"));
    expect(isHeld(screen.getByRole("button", { name: "Send confirmation code" }))).toBe(false);

    rerender(form({ offered: [ContentMode.Off] }));

    expect(radio("Off")).toHaveAttribute("aria-checked", "true");
    expect(radio("On")).toHaveAttribute("aria-checked", "false");
    expect(isHeld(save())).toBe(true);
  });

  // The server refuses anyone else; the form shows them the answer as text,
  // since a group of closed radios is dimmed and out of the tab order.
  it("shows anyone but the owner the mode as text, and who may change it", () => {
    render(form({ mode: ContentMode.On, offered: [ContentMode.Off], canEdit: false }));
    expect(screen.getByText("Content mode:").parentElement).toHaveTextContent(
      "Content mode: On",
    );
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(
      screen.getByText("Only the organization owner can change what this project keeps."),
    ).toBeInTheDocument();
  });

  it("follows a mode changed elsewhere, dropping what the form held", async () => {
    updateContentModeAction.mockResolvedValue({ ok: false, error: "the mode changed" });
    const offered = [ContentMode.Off, ContentMode.On];
    const { rerender } = render(form({ mode: ContentMode.On, offered }));
    fireEvent.click(radio("Off"));
    await act(async () => {
      fireEvent.click(save());
    });
    expect(screen.getByText("the mode changed")).toBeInTheDocument();

    // What a refresh after the owner changed it in another tab looks like:
    // a fresh form, without the refused choice or its refusal.
    rerender(form({ mode: ContentMode.Off, offered }));

    expect(screen.queryByText("the mode changed")).toBeNull();
    expect(radio("Off")).toHaveAttribute("aria-checked", "true");
    expect(isHeld(save())).toBe(true);
  });
});
