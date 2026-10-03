import { describe, expect, it } from "vitest";

import {
  badgeCount,
  bellLabel,
  inviteSubtitle,
  inviterText,
} from "@/lib/notifications";
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

  // `scope` is a free string on the wire. Anything unrecognised is a project
  // rather than echoed into the interface.
  it("treats anything that is not the organization as a project", () => {
    expect(inviteSubtitle(invite({ scope: "project" }))).toBe("Project · Admin");
    expect(inviteSubtitle(invite({ scope: "wat" }))).toBe("Project · Admin");
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
