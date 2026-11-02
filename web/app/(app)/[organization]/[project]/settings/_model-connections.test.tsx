// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("./actions", () => ({
  saveModelConnectionAction: vi.fn(async () => ({ ok: true })),
  deleteModelConnectionAction: vi.fn(async () => ({ ok: true })),
}));

import { ModelConnections, modelsOf } from "./_model-connections";

const CONNECTION = {
  id: "5d4f5c68-0000-4000-8000-0000000000c1",
  name: "Gateway",
  provider: "openai_compatible" as const,
  base_url: "https://gw.example/v1",
  region: null,
  models: ["judge-1"],
  is_default: true,
  secret_hint: "WXYZ",
  created_at: "2026-11-02T00:00:00Z",
  updated_at: "2026-11-02T00:00:00Z",
};

describe("ModelConnections", () => {
  it("reads the models typed by commas or lines, each once", () => {
    expect(modelsOf(" gpt-a, gpt-b\ngpt-a ,, ")).toEqual(["gpt-a", "gpt-b"]);
  });

  it("offers a member no change, and an owner a form for a new one that asks for its key", () => {
    const { rerender } = render(<ModelConnections projectId="p" connections={[CONNECTION]} canEdit={false} />);
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Connect a model" })).toBeNull();

    rerender(<ModelConnections projectId="p" connections={[CONNECTION]} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Connect a model" }));
    const form = screen.getByRole("form", { name: "Connect a model" });
    expect(form).toHaveTextContent("API key");
    expect(screen.getByRole("button", { name: "Connect" })).toBeDisabled();
  });

  it("edits a connection keeping its key unless a new one is typed", () => {
    render(<ModelConnections projectId="p" connections={[CONNECTION]} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const form = screen.getByRole("form", { name: "Edit Gateway" });
    expect(form).toHaveTextContent("leave empty to keep the key");
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });
});
