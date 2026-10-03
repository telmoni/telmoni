// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { IncomingInvite } from "@/lib/types/incoming-invite";

vi.mock("@/components/paper-shell", () => ({
  PaperShell: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));
vi.mock("@/components/session-heartbeat", () => ({
  SessionHeartbeat: () => <div data-testid="heartbeat" />,
}));
vi.mock("./account/privacy/_delete-account", () => ({
  DeleteAccountForm: ({
    email,
    ownedOrganizations,
  }: {
    email: string;
    ownedOrganizations: readonly string[];
  }) => (
    <div data-testid="delete-account" data-email={email} data-owned={ownedOrganizations.join("|")} />
  ),
}));
vi.mock("./account/notifications/_incoming-invites", () => ({
  IncomingInvitesSection: ({ invites }: { invites: readonly IncomingInvite[] }) => (
    <div data-testid="invitations" data-count={invites.length} />
  ),
}));
vi.mock("./account/privacy/_deleted-organizations", () => ({
  DeletedOrganizations: ({ organizations }: { organizations: readonly DeletedOrganization[] }) => (
    <div data-testid="deleted-organizations" data-count={organizations.length} />
  ),
}));

vi.mock("./account/privacy/_sessions", () => ({
  ActiveSessions: ({
    sessions,
    currentId,
  }: {
    sessions: readonly ActiveSession[] | null;
    currentId: string | null;
  }) => (
    <div
      data-testid="sessions"
      data-count={sessions?.length ?? "unavailable"}
      data-current={currentId ?? ""}
    />
  ),
}));

vi.mock("./account/settings/_email", () => ({
  ChangeEmail: ({ email }: { email: string }) => (
    <div data-testid="change-email" data-email={email} />
  ),
}));
vi.mock("./account/settings/_password", () => ({
  PasswordReset: () => <div data-testid="password-reset" />,
}));
vi.mock("./account/privacy/_analytics", () => ({
  AnalyticsPreference: ({ optIn }: { optIn: boolean }) => (
    <div data-testid="analytics" data-opt-in={String(optIn)} />
  ),
}));

import type { ActiveSession, DeletedOrganization } from "@/lib/server/data";

import { AccountOnly } from "./_account-only";

const INVITE = { id: "inv_1" } as IncomingInvite;
const DELETED: DeletedOrganization = {
  organizationId: "org_mine",
  name: "Acme",
  deletionRequestedAt: "2026-09-23T10:00:00Z",
  eraseAfter: "2026-10-07T10:00:00Z",
  restorable: true,
};

function mount(over: Partial<Parameters<typeof AccountOnly>[0]> = {}) {
  return render(
    <AccountOnly
      reason="no-organization"
      email="ada@example.test"
      signupsOpen={false}
      invites={[]}
      ownedOrganizations={[]}
      expiresAt={Date.now() + 60_000}
      {...over}
    />,
  );
}

// ⚠ **A gate closes the product, never the way out.** Whatever put somebody
// here, what they are owed is on the page: their invitations, their
// account's deletion, and sign-out.
describe("AccountOnly", () => {
  for (const reason of ["no-organization", "not-in-beta"] as const) {
    it(`offers the account's deletion and sign-out: ${reason}`, () => {
      mount({ reason, ownedOrganizations: ["Acme"] });
      expect(screen.getByTestId("delete-account")).toHaveAttribute("data-owned", "Acme");
      expect(screen.getByTestId("delete-account")).toHaveAttribute(
        "data-email",
        "ada@example.test",
      );
      expect(screen.getByRole("link", { name: /sign out/i })).toHaveAttribute(
        "href",
        "/auth/logout",
      );
      expect(screen.getByTestId("heartbeat")).toBeInTheDocument();
    });

    it(`offers the invitations waiting for them: ${reason}`, () => {
      mount({ reason, invites: [INVITE] });
      expect(screen.getByTestId("invitations")).toHaveAttribute("data-count", "1");
    });
  }

  // ⚠ This screen stands in for every page, the privacy page included, so
  // ending a stolen session must be on it — for somebody in no organization
  // most of all, who has nowhere else to do it.
  it("lists the browsers they are signed in on, marking this one, for either reason", () => {
    const SESSION = {
      id: "sess_1",
      user_agent: null,
      created_at: "2026-09-23T10:00:00Z",
      last_seen_at: "2026-09-23T10:00:00Z",
    };
    for (const reason of ["no-organization", "not-in-beta"] as const) {
      const { unmount } = mount({ reason, sessions: [SESSION], currentSessionId: "sess_1" });
      expect(screen.getByRole("heading", { name: /active sessions/i })).toBeInTheDocument();
      expect(screen.getByTestId("sessions")).toHaveAttribute("data-count", "1");
      expect(screen.getByTestId("sessions")).toHaveAttribute("data-current", "sess_1");
      unmount();
    }
  });

  it("offers the account's address, password and analytics consent, for either reason", () => {
    for (const reason of ["no-organization", "not-in-beta"] as const) {
      const { unmount } = mount({ reason, authMethod: "password", analyticsOptIn: true });
      expect(screen.getByTestId("change-email")).toHaveAttribute(
        "data-email",
        "ada@example.test",
      );
      expect(screen.getByTestId("password-reset")).toBeInTheDocument();
      expect(screen.getByTestId("analytics")).toHaveAttribute("data-opt-in", "true");
      unmount();
    }
  });

  it("offers no address or password control to a provider sign-in, and still the consent", () => {
    mount({ authMethod: "GoogleOAuth" });
    expect(screen.queryByTestId("change-email")).toBeNull();
    expect(screen.queryByTestId("password-reset")).toBeNull();
    expect(screen.getByTestId("analytics")).toHaveAttribute("data-opt-in", "false");
  });

  it("shows no invitations section when none are waiting", () => {
    mount();
    expect(screen.queryByTestId("invitations")).toBeNull();
  });

  it("offers the organizations they deleted and may restore, for either reason", () => {
    for (const reason of ["no-organization", "not-in-beta"] as const) {
      const { unmount } = mount({ reason, deletedOrganizations: [DELETED] });
      expect(screen.getByTestId("deleted-organizations")).toHaveAttribute("data-count", "1");
      expect(screen.getByRole("heading", { name: /deleted organizations/i })).toBeInTheDocument();
      unmount();
    }
  });

  it("shows no deleted organizations section when none is being deleted", () => {
    mount();
    expect(screen.queryByTestId("deleted-organizations")).toBeNull();
  });

  it("says sign-ups are closed, and that an invitation is the way in", () => {
    mount();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "You're not in an organization",
    );
    expect(document.body.textContent).toMatch(/sign-ups are closed right now/i);
    expect(document.body.textContent).toMatch(/ada@example\.test/);
  });

  it("tells somebody behind the beta wall that an invitation lets them in", () => {
    mount({ reason: "not-in-beta" });
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Not in the beta yet");
    expect(document.body.textContent).toMatch(/invitation from an organization in the beta/i);
  });

  it("tells somebody left without one, while sign-ups are open, that a reload starts one", () => {
    mount({ signupsOpen: true });
    expect(document.body.textContent).toMatch(/reload this page to start a new organization/i);
    expect(document.body.textContent).not.toMatch(/sign-ups are closed/i);
  });
});
