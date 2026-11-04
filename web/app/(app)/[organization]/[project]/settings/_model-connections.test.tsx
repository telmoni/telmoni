// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("./actions", () => ({
  saveModelConnectionAction: vi.fn(async () => ({ ok: true })),
  deleteModelConnectionAction: vi.fn(async () => ({ ok: true })),
}));

// A native select stands in for Radix's, whose listbox jsdom cannot open.
vi.mock("@/components/ui/select", async () => {
  const { Children } = await import("react");
  const SelectTrigger = (_: { id?: string; children?: React.ReactNode }) => null;
  return {
    Select: ({
      value,
      onValueChange,
      disabled,
      children,
    }: {
      value: string;
      onValueChange: (v: string) => void;
      disabled?: boolean;
      children: React.ReactNode;
    }) => {
      const parts = Children.toArray(children) as React.ReactElement<{ id?: string; children?: React.ReactNode }>[];
      const trigger = parts.find((part) => part.type === SelectTrigger);
      return (
        <select id={trigger?.props.id} value={value} disabled={disabled} onChange={(e) => onValueChange(e.target.value)}>
          {parts.filter((part) => part !== trigger)}
        </select>
      );
    },
    SelectTrigger,
    SelectValue: () => null,
    SelectContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    SelectItem: ({ value, children }: { value: string; children: React.ReactNode }) => <option value={value}>{children}</option>,
  };
});

import { ModelConnections, federatedSecret, modelsOf } from "./_model-connections";

const CONNECTION = {
  id: "5d4f5c68-0000-4000-8000-0000000000c1",
  name: "Gateway",
  provider: "openai_compatible" as const,
  base_url: "https://gw.example/v1",
  region: null,
  models: ["judge-1"],
  is_default: true,
  secret_hint: "WXYZ",
  signs_in: false,
  refused_at: null,
  refused_reason: null,
  last_used_at: null,
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

  it("asks for the key again when the endpoint moves, and keeps the provider, so a saved key goes nowhere new", () => {
    render(<ModelConnections projectId="p" connections={[CONNECTION]} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("Provider")).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Base URL"), { target: { value: "https://elsewhere.example/v1" } });
    expect(screen.getByRole("form", { name: "Edit Gateway" })).toHaveTextContent("a new endpoint takes the key again");
  });

  it("shows when a key was last used, and a connection that keeps no key as whom it signs in as", () => {
    render(
      <ModelConnections
        projectId="p"
        connections={[
          { ...CONNECTION, last_used_at: "2026-11-03T08:00:00Z" },
          { ...CONNECTION, id: "c2", name: "Bedrock", provider: "bedrock", base_url: null, region: "us-east-1", signs_in: true, secret_hint: "role/telmoni" },
        ]}
        canEdit={false}
      />,
    );
    expect(screen.getByText(/Last used/)).toBeInTheDocument();
    expect(screen.getByText("Never used")).toBeInTheDocument();
    expect(screen.getByText(/signs in as role\/telmoni, no key kept/)).toBeInTheDocument();
  });

  it("offers signing in as the project only where the deployment signs, and saves whom it signs in as", () => {
    const federation = { issuer: "https://api.example/oidc/v1/projects/p", subject: "project:p", aws_audience: "sts.amazonaws.com" };
    render(<ModelConnections projectId="p" connections={[]} federation={federation} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Connect a model" }));
    expect(screen.queryByLabelText("Signs in with")).toBeNull();
    expect(federatedSecret("bedrock", { role: " arn:aws:iam::123456789012:role/x ", pool: "", cloudProject: "", account: "" })).toBe(
      JSON.stringify({ role_arn: "arn:aws:iam::123456789012:role/x" }),
    );
    expect(federatedSecret("vertex_ai", { role: "", pool: "projects/1/locations/global/workloadIdentityPools/p/providers/r", cloudProject: "acme", account: "" })).toBe(
      JSON.stringify({ workload_identity_provider: "projects/1/locations/global/workloadIdentityPools/p/providers/r", project_id: "acme" }),
    );
    expect(federatedSecret("vertex_ai", { role: "", pool: "", cloudProject: "acme", account: "" })).toBe("");
  });

  it("takes the new key or role when a connection switches how it signs in", () => {
    const federation = { issuer: "https://api.example/oidc/v1/projects/p", subject: "project:p", aws_audience: "sts.amazonaws.com" };
    const keyless = { ...CONNECTION, name: "Bedrock", provider: "bedrock" as const, base_url: null, region: "us-east-1", signs_in: true, secret_hint: "role/telmoni" };
    render(<ModelConnections projectId="p" connections={[keyless]} federation={federation} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText(/Role ARN — leave empty to keep the role/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();

    fireEvent.change(screen.getByLabelText("Signs in with"), { target: { value: "key" } });
    expect(screen.getByLabelText("Secret access key")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Access key id"), { target: { value: "AKIA" } });
    fireEvent.change(screen.getByLabelText("Secret access key"), { target: { value: "s3cr3t" } });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("edits a connection keeping its key unless a new one is typed", () => {
    render(<ModelConnections projectId="p" connections={[CONNECTION]} canEdit />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const form = screen.getByRole("form", { name: "Edit Gateway" });
    expect(form).toHaveTextContent("leave empty to keep the key");
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });
});
