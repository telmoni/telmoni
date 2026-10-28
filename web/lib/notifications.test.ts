import { describe, expect, it } from "vitest";

import {
  badgeCount,
  bellItems,
  bellLabel,
  inviteSubtitle,
  inviterText,
  giverText,
  seatSubtitle,
} from "@/lib/notifications";
import type { OrganizationEntry, ProjectOffer } from "@/lib/server/entities/organization";
import type { AuditExport } from "@/lib/types/audit-export";
import type { GivenSeat } from "@/lib/types/given-seat";
import type { IncomingInvite } from "@/lib/types/incoming-invite";

function invite(over: Partial<IncomingInvite> = {}): IncomingInvite {
  return {
    id: "inv_1",
    scope: "project",
    targetId: "project_1",
    targetName: "Platform",
    role: "admin",
    inviterEmail: "ada@example.com",
    inviterDisplayName: null,
    expiresAt: "2026-10-01T00:00:00Z",
    createdAt: "2026-09-01T00:00:00Z",
    ...over,
  };
}

describe("bellLabel", () => {
  // The badge is aria-hidden, so if the label drops the count a screen reader
  // is told there are notifications and never how many.
  it("carries the count, because the badge is decoration", () => {
    expect(bellLabel(3)).toBe("Notifications: 3 pending items");
  });

  // The count takes in ownership offers as well as invitations, so calling
  // the lot "invitations" would misname what is waiting.
  it("says item, singular, for one", () => {
    expect(bellLabel(1)).toBe("Notifications: 1 pending item");
  });

  it("is the bare noun at zero", () => {
    expect(bellLabel(0)).toBe("Notifications");
    expect(bellLabel(-1)).toBe("Notifications");
  });
});

describe("badgeCount", () => {
  it("renders nothing at zero, so the button keeps its slot unmarked", () => {
    expect(badgeCount(0)).toBeNull();
  });

  it("caps at 9+ rather than widening the button", () => {
    expect(badgeCount(9)).toBe("9");
    expect(badgeCount(10)).toBe("9+");
    expect(badgeCount(240)).toBe("9+");
  });
});

describe("inviteSubtitle", () => {
  it("names the organization level", () => {
    expect(
      inviteSubtitle(invite({ scope: "organization", role: "admin" })),
    ).toBe("Organization · Admin");
  });

  it("names the project level", () => {
    expect(inviteSubtitle(invite({ scope: "project" }))).toBe("Project · Admin");
  });
});

describe("inviterText", () => {
  it("prefers the display name", () => {
    expect(inviterText(invite({ inviterDisplayName: "Ada Lovelace" }))).toBe(
      "Ada Lovelace",
    );
  });

  it("falls back to the address when there is no name", () => {
    expect(inviterText(invite({ inviterDisplayName: null }))).toBe(
      "ada@example.com",
    );
  });

  // Printing both would read as two different people.
  it("does not repeat the address when the name is the address", () => {
    expect(inviterText(invite({ inviterDisplayName: "ada@example.com" }))).toBe(
      "ada@example.com",
    );
  });

  // The invitation outlives a sender who deleted their account, and it is
  // still the organization's to honour — so it is credited to that.
  it("credits the organization when the sender's address is gone", () => {
    expect(inviterText(invite({ inviterEmail: null, inviterDisplayName: null }))).toBe(
      "Platform",
    );
  });

  it("keeps a surviving name when only the address is gone", () => {
    expect(
      inviterText(invite({ inviterEmail: null, inviterDisplayName: "Ada Lovelace" })),
    ).toBe("Ada Lovelace");
  });
});

const SEAT: GivenSeat = {
  projectId: "project_1",
  projectName: "Support agent",
  projectSlug: "support-agent",
  organizationId: "org_1",
  organizationName: "Acme",
  organizationSlug: "acme",
  role: "admin",
  giverEmail: "ada@example.test",
  giverDisplayName: "Ada Lovelace",
  createdAt: "2026-10-21T00:00:00Z",
};

describe("seatSubtitle", () => {
  it("names the organization the project is in, and the role", () => {
    expect(seatSubtitle(SEAT)).toBe("Acme · Admin");
  });
});

describe("giverText", () => {
  it("prefers the giver's name", () => {
    expect(giverText(SEAT)).toBe("Ada Lovelace");
  });

  it("falls back to the address, once, when the name is the address or absent", () => {
    expect(giverText({ ...SEAT, giverDisplayName: null })).toBe("ada@example.test");
    expect(giverText({ ...SEAT, giverDisplayName: "ada@example.test" })).toBe("ada@example.test");
  });

  it("credits the organization when the giver's account is gone", () => {
    expect(giverText({ ...SEAT, giverEmail: null, giverDisplayName: null })).toBe("Acme");
  });
});

function organization(over: Partial<OrganizationEntry> = {}): OrganizationEntry {
  return { organizationId: "org_1", slug: "acme", name: "Acme", role: "owner", ...over };
}

const PROJECT_OFFER: ProjectOffer = {
  projectId: "project_2",
  name: "Billing",
  organizationId: "org_2",
  organizationName: "Globex",
  expiresAt: "2026-11-01T00:00:00Z",
};

const ASKED = {
  format: "csv",
  rangeFrom: null,
  rangeTo: "2026-10-01T00:00:00Z",
  createdAt: "2026-10-01T00:00:00Z",
} as const;

const READY: AuditExport = {
  ...ASKED,
  id: "exp_ready",
  status: "ready",
  rowCount: 10,
  bytes: 100,
  finishedAt: "2026-10-01T00:01:00Z",
  expiresAt: "2026-10-08T00:01:00Z",
  downloadedAt: null,
};
const TAKEN: AuditExport = { ...READY, id: "exp_taken", downloadedAt: "2026-10-02T00:00:00Z" };
const QUEUED: AuditExport = { ...ASKED, id: "exp_queued", status: "queued" };

describe("bellItems", () => {
  const home = organization();

  it("counts what waits on the person, and shows an export still building without counting it", () => {
    const { offers, exportsWaiting, exportsBuilding, count } = bellItems({
      invites: [invite()],
      seats: [SEAT],
      projectOffers: [PROJECT_OFFER],
      organizations: [
        home,
        organization({ organizationId: "org_2", slug: "globex", name: "Globex", role: "member" }),
        organization({
          organizationId: "org_3",
          slug: "initech",
          name: "Initech",
          role: "admin",
          ownershipOfferExpiresAt: "2026-11-01T00:00:00Z",
        }),
      ],
      organization: home,
      auditExports: { org_1: [READY, TAKEN, QUEUED] },
    });
    expect(offers.map((o) => o.organizationId)).toEqual(["org_3"]);
    expect(exportsWaiting.map((e) => e.id)).toEqual(["exp_ready"]);
    expect(exportsBuilding.map((e) => e.id)).toEqual(["exp_queued"]);
    expect(count).toBe(5);
  });

  it("holds no export of another organization, nor any for a member, who has no audit log", () => {
    const elsewhere = bellItems({
      invites: [],
      seats: [],
      projectOffers: [],
      organizations: [home],
      organization: home,
      auditExports: { org_2: [READY] },
    });
    expect(elsewhere.exportsWaiting).toEqual([]);
    expect(elsewhere.count).toBe(0);
    const member = bellItems({
      invites: [],
      seats: [],
      projectOffers: [],
      organizations: [organization({ role: "member" })],
      organization: organization({ role: "member" }),
      auditExports: { org_1: [READY, QUEUED] },
    });
    expect(member.exportsWaiting).toEqual([]);
    expect(member.exportsBuilding).toEqual([]);
  });

  it("holds nothing outside an organization but what is the person's own", () => {
    const { count } = bellItems({
      invites: [invite()],
      seats: [],
      projectOffers: [],
      organizations: [],
      organization: null,
      auditExports: { org_1: [READY] },
    });
    expect(count).toBe(1);
  });
});
