// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  unstable_rethrow: () => {},
}));

// No console shell in a test: the panel is drawn in place.
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: null, cover: () => () => {} }),
}));

const mintTokenAction = vi.fn<(projectId: string, input: unknown) => Promise<unknown>>();
vi.mock("./actions", () => ({
  mintTokenAction: (projectId: string, input: unknown) => mintTokenAction(projectId, input),
}));

import { KeysActions } from "./_manage";

const PROJECT = "project_7bQx2mNv9BcK4dLp";

/** Mint key pressed, and the panel it opens. */
function openPanel() {
  fireEvent.click(screen.getByRole("button", { name: /Mint key/ }));
  return screen.getByRole("dialog");
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("KeysActions", () => {
  it("opens Mint key in the side panel, full over the content area, the name field focused, and shuts on Cancel", () => {
    render(<KeysActions projectId={PROJECT} />);
    expect(screen.queryByRole("dialog")).toBeNull();
    const panel = openPanel();
    expect(panel).toHaveAttribute("data-mode", "full");
    expect(panel).toHaveAttribute("aria-modal", "false");
    expect(within(panel).getByRole("heading", { name: "New API key" })).toBeInTheDocument();
    expect(document.activeElement).toBe(within(panel).getByLabelText("Name"));
    // Both scopes on, and the default expiry.
    expect(within(panel).getByRole("switch", { name: /Read the API/ })).toBeChecked();
    expect(within(panel).getByRole("switch", { name: /Send spans/ })).toBeChecked();
    expect(within(panel).getByRole("combobox", { name: "Expires" })).toHaveTextContent("90 days");
    fireEvent.click(within(panel).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(mintTokenAction).not.toHaveBeenCalled();
  });

  it("mints the key as filled in, shuts the panel, and shows the key once in a dialog", async () => {
    mintTokenAction.mockResolvedValue({ status: "ok", token: "telmoni_abc123" });
    render(<KeysActions projectId={PROJECT} />);
    const panel = openPanel();
    const mint = within(panel).getByRole("button", { name: "Mint key" });
    expect(mint).toBeDisabled();
    fireEvent.change(within(panel).getByLabelText("Name"), { target: { value: "ci-deploy" } });
    fireEvent.change(within(panel).getByLabelText("Description"), { target: { value: "The deploy job" } });
    fireEvent.click(within(panel).getByRole("switch", { name: /Read the API/ }));
    expect(mint).toBeEnabled();
    fireEvent.click(mint);
    await waitFor(() => expect(mintTokenAction).toHaveBeenCalledOnce());
    expect(mintTokenAction).toHaveBeenCalledWith(PROJECT, {
      name: "ci-deploy",
      description: "The deploy job",
      ttl: "90d",
      scopes: ["ingest"],
    });
    const shown = await screen.findByRole("dialog", { name: "Key created" });
    expect(within(shown).getByText("telmoni_abc123")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "New API key" })).toBeNull();
    fireEvent.click(within(shown).getByRole("button", { name: "I've saved it" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("keeps the panel open with the lane's refusal", async () => {
    mintTokenAction.mockResolvedValue({ status: "error", error: "a key by that name exists" });
    render(<KeysActions projectId={PROJECT} />);
    const panel = openPanel();
    fireEvent.change(within(panel).getByLabelText("Name"), { target: { value: "ci-deploy" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Mint key" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent("a key by that name exists");
    expect(within(panel).getByRole("heading", { name: "New API key" })).toBeInTheDocument();
  });

  it("draws nothing for a role that may not mint", () => {
    const { container } = render(<KeysActions projectId={PROJECT} canManage={false} />);
    expect(container).toBeEmptyDOMElement();
  });
});
