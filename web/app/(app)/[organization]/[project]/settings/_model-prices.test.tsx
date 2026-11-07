// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const toastSuccess = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: toastSuccess } }));

const save = vi.hoisted(() => vi.fn());
vi.mock("./actions", () => ({
  saveModelPricesAction: (projectId: string, input: unknown) => save(projectId, input),
}));

import { ModelPricesForm, pricesOf } from "./_model-prices";

const PRICE = {
  provider: "openai",
  models: ["ft:gpt-5:acme", "gpt-5"],
  effective_from: null,
  input: 1.25,
  output: 10,
  cache_read: 0.125,
  cache_write: 0,
};

beforeEach(() => {
  save.mockReset();
  toastSuccess.mockReset();
});

describe("pricesOf", () => {
  it("reads each draft as the lane takes it, and refuses one with no model or a price that is no amount", () => {
    const draft = {
      provider: " openai ",
      models: "ft:gpt-5:acme, gpt-5,",
      effective_from: "",
      input: "1.25",
      output: "10",
      cache_read: "0.125",
      cache_write: "0",
    };
    expect(pricesOf([draft])).toEqual({ prices: [PRICE] });
    expect(pricesOf([{ ...draft, models: " , " }])).toHaveProperty("error");
    expect(pricesOf([{ ...draft, input: "-1" }])).toHaveProperty("error");
    expect(pricesOf([{ ...draft, output: "" }])).toHaveProperty("error");
  });
});

describe("ModelPricesForm", () => {
  it.each([{ prices: [] }, { prices: [PRICE] }])("disables Save for an unchanged list: $prices.length prices", ({ prices }) => {
    render(<ModelPricesForm projectId="project_1" prices={prices} canEdit />);
    const submit = screen.getByRole("button", { name: "Save" });
    expect(submit).toBeDisabled();
    fireEvent.submit(submit.closest("form")!);
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Add a price" }));
    expect(submit).toBeEnabled();
    const added = screen.getByRole("group", { name: `Price ${prices.length + 1}` });
    fireEvent.click(within(added).getByRole("button", { name: "Remove" }));
    expect(submit).toBeDisabled();
  });

  it("disables Save again when an edit is reverted", () => {
    render(<ModelPricesForm projectId="project_1" prices={[PRICE]} canEdit />);
    const input = screen.getByLabelText(/^Input/);
    fireEvent.change(input, { target: { value: "2" } });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    fireEvent.change(input, { target: { value: "1.25" } });
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("shows a member the prices and no way to change them", () => {
    render(<ModelPricesForm projectId="project_1" prices={[PRICE]} canEdit={false} />);
    expect(screen.getByLabelText("Provider")).toHaveValue("openai");
    expect(screen.getByLabelText("Provider")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.getByText(/Only a project owner or admin/)).toBeInTheDocument();
  });

  it("saves the whole list as typed, a row added and one removed", async () => {
    save.mockResolvedValue({ ok: true });
    render(<ModelPricesForm projectId="project_1" prices={[PRICE]} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Add a price" }));
    const added = screen.getByRole("group", { name: "Price 2" });
    fireEvent.change(within(added).getByLabelText("Provider"), { target: { value: "self" } });
    fireEvent.change(within(added).getByLabelText("Models"), { target: { value: "llama-ours" } });
    fireEvent.change(within(added).getByLabelText(/^Input/), { target: { value: "0.2" } });
    fireEvent.change(within(added).getByLabelText(/^Output/), { target: { value: "0.4" } });
    fireEvent.click(
      within(screen.getByRole("group", { name: "Price 1" })).getByRole("button", { name: "Remove" }),
    );
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });
    expect(save).toHaveBeenCalledWith("project_1", {
      prices: [
        {
          provider: "self",
          models: ["llama-ours"],
          effective_from: null,
          input: 0.2,
          output: 0.4,
          cache_read: 0,
          cache_write: 0,
        },
      ],
    });
    expect(toastSuccess).toHaveBeenCalled();
  });

  it("offers Cancel once the list is changed, putting back the saved prices and sending nothing", () => {
    render(<ModelPricesForm projectId="project_1" prices={[PRICE]} canEdit />);
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Add a price" }));
    fireEvent.click(
      within(screen.getByRole("group", { name: "Price 1" })).getByRole("button", { name: "Remove" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getAllByRole("group")).toHaveLength(1);
    expect(within(screen.getByRole("group", { name: "Price 1" })).getByLabelText("Provider")).toHaveValue(
      PRICE.provider,
    );
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    expect(save).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("says why a list cannot be sent, and sends nothing", async () => {
    render(<ModelPricesForm projectId="project_1" prices={[]} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Add a price" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });
    expect(save).not.toHaveBeenCalled();
    expect(screen.getByText(/names its provider and at least one model/)).toBeInTheDocument();
  });
});
