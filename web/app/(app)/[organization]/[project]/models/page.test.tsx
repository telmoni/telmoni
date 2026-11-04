// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { Role } from "@/lib/types/enums";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {action}
    </div>
  ),
}));
vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("@/components/local-time", () => ({ LocalTime: ({ iso }: { iso: string }) => <span>{iso}</span> }));
vi.mock("./_models", async (actual) => ({
  ...(await actual<typeof import("./_models")>()),
  ModelsActions: () => <button type="button">Connect model</button>,
  ModelRowActions: ({ connection }: { connection: { name: string } }) => (
    <button type="button">Edit {connection.name}</button>
  ),
}));

let mockRole: Role;
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({
    kind: "ok",
    gate: {},
    project: { id: "project_7bQx2mNv9BcK4dLp", slug: "my-project", name: "My Project", role: mockRole },
  }),
}));

let mockAnswer: unknown;
vi.mock("@/lib/server/entities/model-connections", () => ({
  fetchModelConnectionsAnswer: async () => mockAnswer,
}));

import ModelsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "my-project" });
const page = async () => render(await ModelsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));

const CONNECTION = {
  id: "5d4f5c68-0000-4000-8000-0000000000c1",
  name: "Gateway",
  provider: "openai_compatible",
  base_url: "https://gw.example/v1",
  region: null,
  models: ["judge-1", "judge-2"],
  is_default: true,
  secret_hint: "WXYZ",
  signs_in: false,
  refused_at: null,
  refused_reason: null,
  last_used_at: null,
  created_at: "2026-11-02T00:00:00Z",
  updated_at: "2026-11-02T00:00:00Z",
};

beforeEach(() => {
  mockRole = Role.Owner;
  mockAnswer = { connections: [CONNECTION], federation: null };
});

describe("ModelsPage", () => {
  // Each connection by its provider, its models and its key's last four, never the key.
  it("lists the connections, the default marked, with the page's action and each row's", async () => {
    await page();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Models");
    expect(screen.getByRole("button", { name: "Connect model" })).toBeInTheDocument();
    const row = screen.getByRole("row", { name: /Gateway/ });
    expect(row).toHaveTextContent("Default");
    expect(row).toHaveTextContent("OpenAI-compatible endpoint");
    expect(row).toHaveTextContent("judge-1, judge-2");
    expect(row).toHaveTextContent("…WXYZ");
    expect(row).toHaveTextContent("Never");
    expect(screen.getByRole("button", { name: "Edit Gateway" })).toBeInTheDocument();
  });

  it("offers a member who is no admin neither action", async () => {
    mockRole = Role.Member;
    await page();
    expect(screen.getByText("Gateway")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connect model" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit Gateway" })).toBeNull();
  });

  it("says when none is connected, and when a key was refused", async () => {
    mockAnswer = { connections: [], federation: null };
    const { unmount } = await page();
    expect(screen.getByText("No model is connected.")).toBeInTheDocument();
    unmount();
    mockAnswer = { connections: [{ ...CONNECTION, refused_at: "2026-11-03T00:00:00Z" }], federation: null };
    await page();
    expect(screen.getByText(/Key refused/)).toBeInTheDocument();
  });

  it("shows a connection that keeps no key as whom it signs in as, and when a key was last used", async () => {
    mockAnswer = {
      connections: [
        { ...CONNECTION, last_used_at: "2026-11-03T08:00:00Z" },
        { ...CONNECTION, id: "c2", name: "Bedrock", provider: "bedrock", signs_in: true, secret_hint: "role/telmoni" },
      ],
      federation: null,
    };
    await page();
    expect(screen.getByRole("row", { name: /Gateway/ })).toHaveTextContent("2026-11-03T08:00:00Z");
    expect(screen.getByRole("row", { name: /Bedrock/ })).toHaveTextContent("Signs in as role/telmoni, no key kept");
  });

  it("says so when the connections could not be read", async () => {
    mockAnswer = null;
    await page();
    expect(screen.getByText(/could not be read/)).toBeInTheDocument();
  });
});
