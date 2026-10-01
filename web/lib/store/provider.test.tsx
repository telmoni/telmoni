// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { useEffect } from "react";

import { StoreProvider } from "./provider";
import {
  useActiveOrganizationId,
  useOrganizations,
  useAddIncomingInvite,
  useUser,
  useIncomingInvites,
  useProjects,
  useProjectsElsewhere,
  useRemoveIncomingInvite,
  useRenameProject,
} from "./index";
import { organizationLabel } from "@/lib/identity";
import type { IncomingInvite, OrganizationEntry } from "@/lib/server/entities/organization";
import type { Project, ProjectEverywhere } from "@/lib/server/entities/projects";

function organization(who: string): OrganizationEntry {
  return {
    organizationId: `org_${who}`,
    name: null,
    ownerEmail: `${who}@example.test`,
    ownerDisplayName: null,
    role: "owner",
  };
}

// The entry for the organization the console is acting in, found the way
// every reader of the store finds it.
function ActiveOrganization() {
  const organizations = useOrganizations();
  const activeId = useActiveOrganizationId();
  const active = organizations.find((o) => o.organizationId === activeId) ?? null;
  return <span data-testid="organization">{active ? organizationLabel(active) : "none"}</span>;
}

function project(id: string, name: string): Project {
  return { id, name, role: "owner" } as Project;
}

function ProjectNames() {
  const names = useProjects()
    .map((p) => p.name)
    .join(", ");
  return <span data-testid="projects">{names || "none"}</span>;
}

function RenameButton({ id, to }: { id: string; to: string }) {
  const rename = useRenameProject();
  return (
    <button type="button" onClick={() => rename(id, to)}>
      rename
    </button>
  );
}

describe("StoreProvider", () => {
  it("seeds the store from server-fetched props", () => {
    render(
      <StoreProvider
        organizations={[organization("alpha")]}
        activeOrganizationId="org_alpha"
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
      >
        <ActiveOrganization />
      </StoreProvider>,
    );
    expect(screen.getByTestId("organization").textContent).toBe("alpha@example.test");
  });

  it("finds the active organization in the list, and none it does not hold", () => {
    const at = (id: string) => (
      <StoreProvider
        organizations={[organization("alpha"), organization("acme")]}
        activeOrganizationId={id}
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
      >
        <ActiveOrganization />
      </StoreProvider>
    );
    const view = render(at("org_acme"));
    expect(screen.getByTestId("organization").textContent).toBe("acme@example.test");

    view.unmount();
    render(at("org_gone"));
    expect(screen.getByTestId("organization").textContent).toBe("none");
  });

  // ⚠ The store is built once per mount, so everything the layout refetches
  // on a client-side navigation has to be written back, or the console renders
  // one server render's answer beside another's. An organization's name moves
  // on a rename and its owner's address on an email change or a hand-over;
  // `user` is the one that genuinely cannot, because signing in is a fresh mount.
  it("takes a renamed organization from refreshed server props", () => {
    const { rerender } = render(
      <StoreProvider
        organizations={[organization("alpha")]}
        activeOrganizationId="org_alpha"
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
      >
        <ActiveOrganization />
      </StoreProvider>,
    );
    rerender(
      <StoreProvider
        organizations={[{ ...organization("alpha"), name: "Alpha Labs" }]}
        activeOrganizationId="org_alpha"
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
      >
        <ActiveOrganization />
      </StoreProvider>,
    );
    expect(screen.getByTestId("organization").textContent).toBe("Alpha Labs");
  });
});

describe("the project list, which is the one slice that changes", () => {
  it("takes a renamed project from refreshed server props", () => {
    const { rerender } = render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("p1", "Personal project")]}
      >
        <ProjectNames />
      </StoreProvider>,
    );
    expect(screen.getByTestId("projects").textContent).toBe("Personal project");

    rerender(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("p1", "New Project")]}
      >
        <ProjectNames />
      </StoreProvider>,
    );
    expect(screen.getByTestId("projects").textContent).toBe("New Project");
  });

  it("renames in place without waiting for the server", () => {
    render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("p1", "Personal project"), project("p2", "Other")]}
      >
        <ProjectNames />
        <RenameButton id="p1" to="New Project" />
      </StoreProvider>,
    );

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "rename" }));
    });
    expect(screen.getByTestId("projects").textContent).toBe(
      "New Project, Other",
    );
  });

  it("ignores a rename for an id that is not in the list", () => {
    render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("p1", "Personal project")]}
      >
        <ProjectNames />
        <RenameButton id="p-gone" to="Ghost" />
      </StoreProvider>,
    );

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "rename" }));
    });
    expect(screen.getByTestId("projects").textContent).toBe("Personal project");
  });
});

function IncomingInviteCount() {
  const invites = useIncomingInvites();
  return <span data-testid="invites-count">{invites.length}</span>;
}

function RemoveInviteButton({ id }: { id: string }) {
  const remove = useRemoveIncomingInvite();
  return (
    <button type="button" onClick={() => remove(id)}>
      remove
    </button>
  );
}

function sampleInvite(id: string): IncomingInvite {
  return {
    id,
    scope: "project",
    targetId: "project_test",
    targetName: "Test Project",
    role: "admin",
    inviterEmail: "inviter@example.test",
    inviterDisplayName: null,
    createdAt: "2026-09-12T00:00:00Z",
    expiresAt: "2026-09-19T00:00:00Z",
  };
}

describe("the incoming invites slice", () => {
  it("synchronizes incoming invites from refreshed server props", () => {
    const { rerender } = render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
        incomingInvites={[sampleInvite("inv_1")]}
      >
        <IncomingInviteCount />
      </StoreProvider>,
    );
    expect(screen.getByTestId("invites-count").textContent).toBe("1");

    rerender(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
        incomingInvites={[]}
      >
        <IncomingInviteCount />
      </StoreProvider>,
    );
    expect(screen.getByTestId("invites-count").textContent).toBe("0");
  });

  it("removes an invite optimistically via removeIncomingInvite", () => {
    render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
        incomingInvites={[sampleInvite("inv_1"), sampleInvite("inv_2")]}
      >
        <IncomingInviteCount />
        <RemoveInviteButton id="inv_1" />
      </StoreProvider>,
    );
    expect(screen.getByTestId("invites-count").textContent).toBe("2");

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "remove" }));
    });
    expect(screen.getByTestId("invites-count").textContent).toBe("1");
  });
});

describe("StoreProvider: projects elsewhere", () => {
  function elsewhere(id: string, name: string, organization: string): ProjectEverywhere {
    return {
      id,
      name,
      role: "admin",
      organizationId: organization,
      organizationName: null,
      organizationOwnerEmail: `${organization}@example.test`,
    };
  }

  function ElsewhereNames() {
    const names = useProjectsElsewhere()
      .map((t) => `${t.name}@${t.organizationId}`)
      .join(", ");
    return <span data-testid="elsewhere">{names || "none"}</span>;
  }

  it("is empty when the layout passed nothing — the common case", () => {
    render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
      >
        <ElsewhereNames />
      </StoreProvider>,
    );
    expect(screen.getByTestId("elsewhere").textContent).toBe("none");
  });

  it("seeds from the server and reconciles when the layout hands over a new list", () => {
    const first = [elsewhere("project_a", "Ops", "org_x")];
    const view = render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
        projectsElsewhere={first}
      >
        <ElsewhereNames />
      </StoreProvider>,
    );
    expect(screen.getByTestId("elsewhere").textContent).toBe("Ops@org_x");

    view.rerender(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[]}
        projectsElsewhere={[...first, elsewhere("project_b", "Data", "org_y")]}
      >
        <ElsewhereNames />
      </StoreProvider>,
    );
    expect(screen.getByTestId("elsewhere").textContent).toBe("Ops@org_x, Data@org_y");
  });
});

// ⚠ **THE BUG THIS SHIPPED WITH.** `createStore` runs once per MOUNT, and
// switching organizations is a client-side navigation inside the same layout —
// so `projects` was reseeded from the organization you switched into while
// `activeOrganizationId` kept the value it was born with. The resource selector
// reads the two together, and drew somebody else's project under the name and
// Owner badge of the organization you had left.
describe("StoreProvider: the organization you are standing in", () => {
  function Standing() {
    const active = useActiveOrganizationId() ?? "none";
    const projects = useProjects()
      .map((t) => t.name)
      .join(", ");
    return <span data-testid="standing">{`${active} | ${projects || "none"}`}</span>;
  }

  function at(id: string | null, projects: readonly Project[]) {
    return (
      <StoreProvider
        organizations={[organization("alpha"), organization("acme")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={projects}
        activeOrganizationId={id}
      >
        <Standing />
      </StoreProvider>
    );
  }

  it("follows the layout into another organization, not just its projects", () => {
    const view = render(at("org_alpha", [project("project_a", "Mine")]));
    expect(screen.getByTestId("standing").textContent).toBe("org_alpha | Mine");

    view.rerender(at("org_acme", [project("project_b", "Theirs")]));

    expect(
      screen.getByTestId("standing").textContent,
      "the projects moved and the organization did not — the project is drawn under the wrong one",
    ).toBe("org_acme | Theirs");
  });

  // ⚠ The account menu reads `user.email` straight out of the store, and the
  // email-change flow reseals the cookie and calls `router.refresh()` on a
  // session that keeps rendering for minutes. Without this the menu goes on
  // showing the address the person just moved away from.
  it("follows an email change into the account menu", () => {
    function Email() {
      return <span data-testid="email">{useUser()?.email ?? "none"}</span>;
    }
    const at = (email: string) => (
      <StoreProvider
        organizations={[organization("alpha")]}
        user={{ id: "user_alpha", email, firstName: null, lastName: null }}
        flags={{}}
        roles={{}}
        projects={[]}
      >
        <Email />
      </StoreProvider>
    );
    const view = render(at("old@example.test"));
    expect(screen.getByTestId("email").textContent).toBe("old@example.test");

    view.rerender(at("new@example.test"));

    expect(screen.getByTestId("email").textContent).toBe("new@example.test");
  });

  // `null` is the layout saying `/me` answered nothing. Holding on to the last
  // id instead would go on naming one auth has not vouched for on this render.
  it("keeps an absent id absent, rather than the organization it stood in before", () => {
    const view = render(at("org_acme", []));
    expect(screen.getByTestId("standing").textContent).toBe("org_acme | none");

    view.rerender(at(null, [project("project_a", "Mine")]));

    expect(screen.getByTestId("standing").textContent).toBe("none | Mine");
  });
});

describe("refreshed props that say nothing new", () => {
  let commits = 0;

  function CountingProjectNames() {
    const names = useProjects()
      .map((p) => p.name)
      .join(", ");
    useEffect(() => {
      commits += 1;
    });
    return <span data-testid="projects">{names || "none"}</span>;
  }

  it("leaves consumers alone when the server resends the same project list", () => {
    commits = 0;
    const { rerender } = render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("p1", "Personal project")]}
      >
        <CountingProjectNames />
      </StoreProvider>,
    );
    const afterMount = commits;

    // What every navigation looks like: the layout rebuilt the array and the
    // objects in it, and none of it says anything the store does not have.
    rerender(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("p1", "Personal project")]}
      >
        <CountingProjectNames />
      </StoreProvider>,
    );

    // Once for the rerender itself, and not a second time for a store write.
    expect(commits).toBe(afterMount + 1);
  });
});

describe("a consumer with no provider above it", () => {
  it("throws instead of reading a store shared by every request", () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      expect(() => render(<ActiveOrganization />)).toThrow(/StoreProvider/);
    } finally {
      quiet.mockRestore();
    }
  });
});

describe("the actions the hooks hand out", () => {
  // ⚠ **`realtime-listener.tsx` names these in a dependency array**, and the
  // effect they gate opens the EventSource. An action whose identity moved on
  // every store write would tear down and reopen that connection each time a
  // project was renamed or the server reseeded the list — a reconnect storm with
  // no symptom in this file. Identity is stable because `set` merges a partial
  // over the same functions, and this is the test that says so out loud.
  it("keeps one identity across a store write", () => {
    const seen: { add: unknown; remove: unknown }[] = [];

    function Captures() {
      seen.push({ add: useAddIncomingInvite(), remove: useRemoveIncomingInvite() });
      return <span data-testid="projects-count">{useProjects().length}</span>;
    }

    const view = render(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("a", "Alpha")]}
      >
        <Captures />
        <RenameButton id="a" to="Renamed" />
      </StoreProvider>,
    );

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "rename" }));
    });
    view.rerender(
      <StoreProvider
        organizations={[organization("alpha")]}
        user={null}
        flags={{}}
        roles={{}}
        projects={[project("a", "Alpha"), project("b", "Beta")]}
      >
        <Captures />
        <RenameButton id="a" to="Renamed" />
      </StoreProvider>,
    );

    expect(screen.getByTestId("projects-count"), "the store never changed").toHaveTextContent("2");
    expect(seen.length, "the consumer never re-rendered").toBeGreaterThan(1);
    const first = seen[0]!;
    for (const captured of seen) {
      expect(captured.add, "addIncomingInvite moved").toBe(first.add);
      expect(captured.remove, "removeIncomingInvite moved").toBe(first.remove);
    }
  });
});
