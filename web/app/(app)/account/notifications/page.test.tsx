// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { OrganizationEntry, ProjectOffer } from "@/lib/server/entities/organization";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

// Invitations have their own row and their own actions; what this page owes
// them is only to show the section when there are some.
vi.mock("./_incoming-invites", () => ({
  IncomingInvitesSection: ({ invites }: { invites: readonly unknown[] }) => (
    <div data-testid="pending-invitations" data-count={invites.length} />
  ),
}));

const refresh = vi.hoisted(() => vi.fn());
const push = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh, push }),
}));

const acceptOwnershipAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
);
const declineOwnershipAction = vi.hoisted(() =>
  vi.fn<(organizationId: string) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
);
const acceptProjectOfferAction = vi.hoisted(() =>
  vi.fn<
    (projectId: string, organizationId?: string) => Promise<{ error: string | null; href?: string }>
  >(async () => ({ error: null, href: "/project_offered" })),
);
const declineProjectOfferAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<{ error: string | null }>>(async () => ({
    error: null,
  })),
);
vi.mock("./ownership-actions", () => ({
  acceptOwnershipAction,
  declineOwnershipAction,
  acceptProjectOfferAction,
  declineProjectOfferAction,
}));

let mockContext: {
  organizations: OrganizationEntry[];
  activeOrganizationId: string;
  incomingInvites: unknown[];
  projectOffers?: ProjectOffer[];
} | null = null;
vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
}));

import AccountNotificationsPage from "./page";

const OWN: OrganizationEntry = {
  organizationId: "org_own",
  slug: "own",
  name: null,
  ownerEmail: "admin@example.test",
  role: "owner",
};

const OFFERED: OrganizationEntry = {
  organizationId: "org_offered",
  slug: "analytical-engines",
  name: "Analytical Engines",
  ownerEmail: "charles@example.test",
  ownerDisplayName: "Charles Babbage",
  role: "admin",
  ownershipOfferExpiresAt: "2026-09-30T00:00:00Z",
};

const NOT_OFFERED: OrganizationEntry = {
  organizationId: "org_other",
  slug: "other",
  name: "Difference Engine",
  ownerEmail: "ada@example.test",
  role: "admin",
  ownershipOfferExpiresAt: null,
};

const INVITE = {
  id: "inv_1",
  scope: "organization",
  targetId: "org_beta",
  targetName: "Beta Labs",
  role: "member",
  inviterEmail: "founder@example.test",
  inviterDisplayName: null,
  expiresAt: "2026-09-30T00:00:00Z",
  createdAt: "2026-09-20T00:00:00Z",
};

const PROJECT_OFFERED: ProjectOffer = {
  projectId: "project_offered",
  name: "Payments",
  organizationId: "org_offered",
  organizationName: "Analytical Engines",
  ownerEmail: "charles@example.test",
  ownerDisplayName: "Charles Babbage",
  expiresAt: "2026-10-05T00:00:00Z",
};

// Standing in the caller's own organization: an offer is answered from here
// wherever the console happens to be.
function me(
  organizations: OrganizationEntry[],
  incomingInvites: unknown[] = [],
  projectOffers: ProjectOffer[] = [],
) {
  mockContext = {
    organizations,
    activeOrganizationId: "org_own",
    incomingInvites,
    projectOffers,
  };
}

const press = async (name: string) => {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
};

beforeEach(() => {
  vi.clearAllMocks();
  acceptOwnershipAction.mockResolvedValue({ error: null });
  declineOwnershipAction.mockResolvedValue({ error: null });
  me([OWN, OFFERED, NOT_OFFERED]);
});

describe("AccountNotificationsPage: ownership offers", () => {
  it("lists each organization offered to the caller, and who is offering it", async () => {
    render(await AccountNotificationsPage());
    const offers = screen.getByTestId("ownership-offers");
    expect(
      within(offers).getByText("Charles Babbage wants to hand you Analytical Engines"),
    ).toBeInTheDocument();
    expect(within(offers).getAllByRole("button", { name: "Accept" })).toHaveLength(1);
  });

  // An entry is an offer only while it carries a live expiry. The caller's
  // own organization and one they merely administer are nothing to answer.
  it("lists no organization that has not been offered", async () => {
    render(await AccountNotificationsPage());
    expect(document.body.textContent).not.toContain("Difference Engine");
    expect(screen.getAllByText(/wants to hand you/)).toHaveLength(1);
  });

  it("draws no offers section when nothing has been offered", async () => {
    me([OWN, NOT_OFFERED], [INVITE]);
    render(await AccountNotificationsPage());
    expect(screen.queryByTestId("ownership-offers")).toBeNull();
    expect(screen.getByTestId("pending-invitations")).toBeInTheDocument();
  });

  // ⚠ Accepting hands the caller the organization's roster and its deletion,
  // so it is confirmed first — and it answers for the organization OFFERED,
  // which is not the one the console is standing in.
  it("accepts, once confirmed, for the organization offered", async () => {
    render(await AccountNotificationsPage());
    await press("Accept");
    expect(acceptOwnershipAction, "accepted before it was confirmed").not.toHaveBeenCalled();

    await press("Become the owner");
    expect(acceptOwnershipAction).toHaveBeenCalledWith("org_offered");
    expect(refresh).toHaveBeenCalled();
  });

  it("declines for the organization offered", async () => {
    render(await AccountNotificationsPage());
    await press("Decline");
    expect(declineOwnershipAction).toHaveBeenCalledWith("org_offered");
    expect(acceptOwnershipAction).not.toHaveBeenCalled();
    expect(refresh).toHaveBeenCalled();
  });

  it("shows auth's refusal, and keeps the offer on the page", async () => {
    declineOwnershipAction.mockResolvedValue({ error: "that offer has lapsed" });
    render(await AccountNotificationsPage());
    await press("Decline");
    expect(screen.getByRole("alert")).toHaveTextContent("that offer has lapsed");
    expect(screen.getByTestId("ownership-offers")).toBeInTheDocument();
    expect(refresh).not.toHaveBeenCalled();
  });
});

// A project offered to the caller lands in an organization they own, so the
// page asks which when they own several and says so when they own none.
describe("AccountNotificationsPage: project offers", () => {
  it("lists each project offered to the caller, who is offering it, and where it comes from", async () => {
    me([OWN, NOT_OFFERED], [], [PROJECT_OFFERED]);
    render(await AccountNotificationsPage());
    const offers = screen.getByTestId("project-offers");
    expect(
      within(offers).getByText("Charles Babbage wants to hand you the project Payments"),
    ).toBeInTheDocument();
    expect(offers).toHaveTextContent(/leaves Analytical Engines/);
    expect(screen.queryByTestId("ownership-offers")).toBeNull();
  });

  // ⚠ Accepting takes the project's keys and members into the caller's own
  // organization, so it is confirmed first — and it lands where the caller
  // said, then takes them there.
  it("accepts, once confirmed, into the one organization the caller owns and goes to the project", async () => {
    me([OWN, NOT_OFFERED], [], [PROJECT_OFFERED]);
    render(await AccountNotificationsPage());
    await press("Accept");
    expect(acceptProjectOfferAction, "accepted before it was confirmed").not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(/connectors stay behind/i);
    expect(within(dialog).queryByText("Which organization")).toBeNull();

    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Take it over" }));
    });
    expect(acceptProjectOfferAction).toHaveBeenCalledWith("project_offered", "org_own");
    expect(push).toHaveBeenCalledWith("/project_offered");
  });

  it("asks which organization when the caller owns several", async () => {
    const second: OrganizationEntry = {
      organizationId: "org_second",
      slug: "second",
      name: "Second",
      ownerEmail: "admin@example.test",
      role: "owner",
    };
    me([OWN, second], [], [PROJECT_OFFERED]);
    render(await AccountNotificationsPage());
    await press("Accept");
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Which organization")).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Take it over" }));
    });
    expect(acceptProjectOfferAction).toHaveBeenCalledWith("project_offered", "org_own");
  });

  it("cannot accept owning no organization, and can still decline", async () => {
    me([NOT_OFFERED], [], [PROJECT_OFFERED]);
    render(await AccountNotificationsPage());
    expect(screen.getByTestId("project-offer-nowhere")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Accept" })).toBeNull();

    await press("Decline");
    expect(declineProjectOfferAction).toHaveBeenCalledWith("project_offered");
    expect(refresh).toHaveBeenCalled();
  });

  it("shows auth's refusal, and keeps the offer on the page", async () => {
    declineProjectOfferAction.mockResolvedValue({ error: "that offer has lapsed" });
    me([OWN], [], [PROJECT_OFFERED]);
    render(await AccountNotificationsPage());
    await press("Decline");
    expect(screen.getByRole("alert")).toHaveTextContent("that offer has lapsed");
    expect(screen.getByTestId("project-offers")).toBeInTheDocument();
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("AccountNotificationsPage: when there is nothing", () => {
  it("says so only when there is neither an offer nor an invitation", async () => {
    me([OWN, NOT_OFFERED]);
    const { unmount } = render(await AccountNotificationsPage());
    expect(screen.getByTestId("notifications-empty")).toBeInTheDocument();
    unmount();

    me([OWN, OFFERED]);
    const { unmount: close } = render(await AccountNotificationsPage());
    expect(screen.queryByTestId("notifications-empty"), "an offer is something").toBeNull();
    close();

    me([OWN], [], [PROJECT_OFFERED]);
    const { unmount: shut } = render(await AccountNotificationsPage());
    expect(
      screen.queryByTestId("notifications-empty"),
      "a project offer is something",
    ).toBeNull();
    shut();

    me([OWN], [INVITE]);
    render(await AccountNotificationsPage());
    expect(screen.queryByTestId("notifications-empty"), "an invitation is something").toBeNull();
  });

  it("shows the outage card when auth cannot answer", async () => {
    mockContext = null;
    render(await AccountNotificationsPage());
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
