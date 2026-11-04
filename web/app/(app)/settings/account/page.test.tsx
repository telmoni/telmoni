// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import path from "node:path";

import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("notFound");
  },
}));

vi.mock("./_password", () => ({
  PasswordReset: ({ method }: { method: string | null }) => (
    <div data-testid="password">{method ?? "unknown"}</div>
  ),
}));

vi.mock("./_email", () => ({
  ChangeEmail: ({
    method,
    email,
  }: {
    method: string | null;
    email: string;
  }) => <div data-testid="email">{`${method ?? "unknown"}:${email}`}</div>,
}));

vi.mock("./_default-organization", () => ({
  DefaultOrganization: ({
    organizations,
    current,
  }: {
    organizations: { organizationId: string; label: string }[];
    current: string;
  }) => (
    <div data-testid="default-organization" data-current={current}>
      {organizations.map((o) => `${o.organizationId}:${o.label}`).join(",")}
    </div>
  ),
}));

vi.mock("./_slack", () => ({
  UnlinkSlack: ({ teamId, workspace }: { teamId: string; workspace: string }) => (
    <button type="button" data-testid={`unlink-${teamId}`}>{`Unlink ${workspace}`}</button>
  ),
}));

// `/me`, for the organizations. Unread by default, so every test that is not
// about the default organization sees the page as it stands without one.
const context = vi.hoisted(() => ({
  value: null as {
    organizations: { organizationId: string; name: string | null; role: string }[];
    defaultOrganizationId: string | null;
  } | null,
}));
// The Slack accounts linked. Unread by default, as `/me` is.
const slack = vi.hoisted(() => ({
  value: null as {
    offered: boolean;
    identities: { team_id: string; team_name: string | null; linked_at: string }[];
  } | null,
}));
vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => context.value,
  defaultOrganization: (ctx: NonNullable<typeof context.value>) =>
    ctx.organizations.find((o) => o.organizationId === ctx.defaultOrganizationId) ?? null,
  fetchSlackIdentities: async () => slack.value,
}));

const session = vi.hoisted(() => ({
  value: {
    userId: "user_1",
    email: "k@example.com",
    firstName: "K",
    lastName: null,
    sessionRowId: null,
    authMethod: "google" as string | null,
  },
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => session.value,
}));

beforeEach(() => {
  session.value = { ...session.value, authMethod: "google" };
  context.value = null;
  slack.value = null;
});

import AccountPage from "./page";

describe("AccountPage", () => {
  it("shows who you are", async () => {
    render(await AccountPage());
    expect(screen.getByTestId("page-header")).toHaveTextContent("Account");
    expect(screen.getByText("k@example.com")).toBeInTheDocument();
    expect(screen.getByText("user_1")).toBeInTheDocument();
  });

  it("names how you signed in", async () => {
    render(await AccountPage());
    expect(screen.getByText("Sign-in method")).toBeInTheDocument();
    expect(screen.getByText("Google")).toBeInTheDocument();
  });

  it("draws no row at all when the provider said nothing", async () => {
    session.value = { ...session.value, authMethod: null };
    render(await AccountPage());
    expect(screen.queryByText("Sign-in method")).toBeNull();
    // The closed default has to survive the PAGE, not only the component. A
    // null method may well be a provider sign-up, so it gets what one gets:
    // nothing offered, rather than a control on a guess.
    expect(screen.queryByTestId("password")).toBeNull();
    expect(screen.queryByTestId("email")).toBeNull();
  });

  // ⚠ The pin for the whole rule. A Google account is told how it signs in and
  // offered nothing else — no Email section, no Password section, and no card
  // explaining the absence of either. Restoring the prose means deleting this.
  it("offers a provider account neither control, and no prose about it", async () => {
    render(await AccountPage());
    expect(screen.queryByTestId("email")).toBeNull();
    expect(screen.queryByTestId("password")).toBeNull();
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    expect(headings).toEqual(["profile"]);
    // The Profile row is the breadcrumb that replaced both paragraphs.
    expect(screen.getByText("Google")).toBeInTheDocument();
  });

  it("offers the password control to an email-and-password account", async () => {
    session.value = { ...session.value, authMethod: "password" };
    render(await AccountPage());
    expect(screen.getByTestId("password")).toHaveTextContent("password");
  });

  // Hands over the RAW method and the address from the cookie — not a label,
  // not a boolean. The page decides whether to DRAW the section, because only
  // it can remove the `<Section>` wrapper; what a method means past that is
  // `lib/sign-in-method`'s vocabulary, and the component and the Server Action
  // each ask it themselves.
  it("hands the email control the method and address it was given", async () => {
    session.value = { ...session.value, authMethod: "password" };
    render(await AccountPage());
    expect(screen.getByTestId("email")).toHaveTextContent(
      "password:k@example.com",
    );
  });

  it("offers no way to edit the name the provider owns", async () => {
    render(await AccountPage());
    // Scoped to Profile on purpose. The email control is stubbed here, so a
    // page-wide textbox count is vacuously zero and would protect nothing once
    // this page has a form on it at all.
    const profile = screen
      .getByRole("heading", { level: 2, name: /^profile$/i })
      .closest("section");
    expect(profile).not.toBeNull();
    expect(profile?.querySelectorAll("input, textarea")).toHaveLength(0);
  });

  it("holds who you are and how you get in — no sessions, no erasure", async () => {
    session.value = { ...session.value, authMethod: "password" };
    render(await AccountPage());
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    // Order is the assertion, not just membership: who you are, then the two
    // halves of how you get in, with the address before the password that hangs
    // off it.
    expect(headings).toEqual(["profile", "email", "password"]);
    expect(screen.queryByText(/danger zone/i)).toBeNull();
    expect(screen.queryByText(/active sessions/i)).toBeNull();
  });

  // After how you get in, where signing in opens: chosen among the person's
  // own organizations, starting from the one auth answers now.
  it("offers the default organization last, among the person's own", async () => {
    session.value = { ...session.value, authMethod: "password" };
    context.value = {
      organizations: [
        { organizationId: "org_1", name: "Acme", role: "owner" },
        { organizationId: "org_2", name: "Globex", role: "member" },
      ],
      defaultOrganizationId: "org_2",
    };
    render(await AccountPage());
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    expect(headings).toEqual(["profile", "email", "password", "default organization"]);
    const control = screen.getByTestId("default-organization");
    expect(control).toHaveAttribute("data-current", "org_2");
    // Each by its own name, never a person's.
    expect(control).toHaveTextContent("org_1:Acme,org_2:Globex");
  });

  it("leaves the default out for somebody in no organization", async () => {
    context.value = { organizations: [], defaultOrganizationId: null };
    render(await AccountPage());
    expect(screen.queryByTestId("default-organization")).toBeNull();
    expect(screen.getByText("k@example.com")).toBeInTheDocument();
  });

  it("leaves Slack out where the service cannot link one", async () => {
    slack.value = { offered: false, identities: [] };
    render(await AccountPage());
    expect(screen.queryByRole("heading", { level: 2, name: /^slack$/i })).toBeNull();
  });

  // Last: what acts as you somewhere else, each workspace by its own name
  // with its Unlink, and a plain link that starts the sign-in.
  it("offers Slack last, its accounts by workspace and the link that starts a sign-in", async () => {
    slack.value = {
      offered: true,
      identities: [
        { team_id: "T0001", team_name: "Acme", linked_at: "2026-11-04T10:00:00Z" },
        { team_id: "T0002", team_name: null, linked_at: "2026-11-04T11:00:00Z" },
      ],
    };
    render(await AccountPage());
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    expect(headings).toEqual(["profile", "slack"]);
    expect(screen.getByTestId("unlink-T0001")).toHaveTextContent("Unlink Acme");
    expect(screen.getByTestId("unlink-T0002")).toHaveTextContent("Unlink T0002");
    expect(screen.getByRole("link", { name: "Link another Slack account" })).toHaveAttribute(
      "href",
      "/connect/slack/identity",
    );
    // Where a press refused in Slack sends the presser.
    expect(document.getElementById("slack")).not.toBeNull();
  });

  it.each([
    ["linked", "Your Slack account is linked."],
    ["elsewhere", "That Slack account is linked to another account here. Unlink it there first."],
    ["nonsense", "That sign-in expired or was already used. Start again."],
  ])("says how a link came back, ?slack=%s", async (word, words) => {
    slack.value = { offered: true, identities: [] };
    render(await AccountPage({ searchParams: Promise.resolve({ slack: word }) }));
    expect(screen.getByRole("status")).toHaveTextContent(words);
    expect(screen.getByRole("link", { name: "Link your Slack account" })).toBeInTheDocument();
  });

  // Everything else on the page is the session's, so it stands when `/me`
  // or the Slack accounts cannot be read: those two are all it asks for.
  it("reads nothing but /me and the Slack accounts linked", () => {
    const source = readFileSync(path.join(__dirname, "page.tsx"), "utf8");
    expect(source.match(/\bfetch[A-Z]\w*/g)).toEqual([
      "fetchSlackIdentities",
      "fetchSlackIdentities",
    ]);
  });
});
