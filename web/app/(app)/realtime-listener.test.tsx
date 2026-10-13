// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RealtimeListener } from "@/components/realtime-listener";
import { listDrawn, newTraces } from "@/lib/use-new-traces";

const mockReplace = vi.fn();
const mockRefresh = vi.fn();
let mockPathname = "/acme/web/members";
let mockActiveOrgId: string | null = "org_1";
// The listing of the organization the console stands in. Events name a
// project by id; the path names it by its slug in that organization.
let mockProjects = [
  { id: "project_1", slug: "web" },
  { id: "project_10", slug: "web-2" },
];
const ORGANIZATIONS = [
  { organizationId: "org_1", slug: "acme" },
  { organizationId: "org_2", slug: "globex" },
];

vi.mock("next/navigation", () => ({
  useRouter: () => ({
    replace: mockReplace,
    refresh: mockRefresh,
  }),
  usePathname: () => mockPathname,
}));

vi.mock("@/lib/store", () => ({
  useIncomingInvites: () => [],
  useOrganizations: () => ORGANIZATIONS,
  useProjects: () => mockProjects,
  useActiveOrganizationId: () => mockActiveOrgId,
  useActiveOrganization: () =>
    ORGANIZATIONS.find((o) => o.organizationId === mockActiveOrgId) ?? null,
  useAddIncomingInvite: () => vi.fn(),
  useRemoveIncomingInvite: () => vi.fn(),
}));

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  listeners: Record<string, ((e: MessageEvent) => void)[]> = {};
  readyState = 0;

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(event: string, cb: (e: MessageEvent) => void) {
    this.listeners[event] = this.listeners[event] || [];
    this.listeners[event].push(cb);
  }

  removeEventListener() {}

  close() {
    this.readyState = 2;
  }

  emit(event: string, data: unknown) {
    const list = this.listeners[event] || [];
    for (const cb of list) {
      cb({ data: JSON.stringify(data) } as MessageEvent);
    }
  }

  emitRaw(event: string, raw: string) {
    const list = this.listeners[event] || [];
    for (const cb of list) {
      cb({ data: raw } as MessageEvent);
    }
  }
}

// @ts-expect-error test mock
global.EventSource = MockEventSource;

describe("RealtimeListener - membership:removed edge cases", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    MockEventSource.instances = [];
    mockActiveOrgId = "org_1";
    mockProjects = [
      { id: "project_1", slug: "web" },
      { id: "project_10", slug: "web-2" },
    ];
  });

  it("ejects the user to /console when removed from their active project subpath", () => {
    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).toHaveBeenCalledWith("/console");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("ejects the user to /console when removed from their active project root (/acme/web)", () => {
    mockPathname = "/acme/web";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).toHaveBeenCalledWith("/console");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("does not false-positive match a slug prefix (/acme/web-2 when removed from web)", () => {
    mockPathname = "/acme/web-2/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  // Two organizations may each have a project of the same name: the slug alone
  // says nothing about which project the path is on.
  it("does not eject from a same-named project of another organization", () => {
    mockPathname = "/globex/web";
    mockActiveOrgId = "org_2";
    mockProjects = [{ id: "project_9", slug: "web" }];
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("does not eject from the organization's own pages when only removed from a project in it", () => {
    mockPathname = "/acme/projects";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("ejects the user to /console when removed from the active organization", () => {
    mockPathname = "/acme/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
    });

    expect(mockReplace).toHaveBeenCalledWith("/console");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("does not eject from /account/settings when removed from an organization", () => {
    mockPathname = "/account/settings";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("does not eject when removed from an organization that is not currently active", () => {
    mockPathname = "/globex/members";
    mockActiveOrgId = "org_2";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("safely ignores malformed JSON or unparseable event data", () => {
    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emitRaw("membership:removed", "not-a-json");

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).not.toHaveBeenCalled();
  });
});

// A project handed to another organization leaves the path it was open on
// naming nothing. The event names where it went, by the slugs its new address
// is spelled with.
describe("RealtimeListener - ownership:changed", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    MockEventSource.instances = [];
    mockActiveOrgId = "org_1";
    mockProjects = [
      { id: "project_1", slug: "web" },
      { id: "project_10", slug: "web-2" },
    ];
  });

  it("follows the project on screen into the organization it was handed to", () => {
    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);

    MockEventSource.instances[0].emit("ownership:changed", {
      organizationId: "org_2",
      projectId: "project_1",
      organizationSlug: "globex",
      projectSlug: "web",
    });

    // By slug, as every link is spelled.
    expect(mockReplace).toHaveBeenCalledWith("/globex/web");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  // Without the slugs there is nowhere to spell: the refresh answers "not
  // found" here, which is the truth of this path.
  it("refreshes in place when the event does not say where the project went", () => {
    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);

    MockEventSource.instances[0].emit("ownership:changed", {
      organizationId: "org_2",
      projectId: "project_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalledTimes(1);
  });

  // An offer made, withdrawn or declined names the organization the project
  // is still in: every role on the page may have moved, and nothing else.
  it("refreshes in place when the project on screen stays where it is", () => {
    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);

    MockEventSource.instances[0].emit("ownership:changed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("refreshes in place when another project moved, or the organization changed hands", () => {
    mockPathname = "/acme/web-2";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("ownership:changed", { organizationId: "org_2", projectId: "project_1" });
    es.emit("ownership:changed", { organizationId: "org_2" });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalledTimes(2);
  });
});

// An invitation sent from another tab is a row on whichever roster is on
// screen. The organization's own sits a segment higher than a project's.
describe("RealtimeListener - rosters", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    MockEventSource.instances = [];
    mockActiveOrgId = "org_1";
  });

  it.each(["/acme/members", "/acme/audit-log", "/acme/web/members", "/acme/web/audit-log"])(
    "refreshes %s when an invitation is sent",
    (pathname) => {
      mockPathname = pathname;
      render(<RealtimeListener />);
      MockEventSource.instances[0].emit("invite:sent", {});
      expect(mockRefresh).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["/acme", "/acme/settings", "/acme/web", "/acme/web/api-keys", "/account/settings"])(
    "leaves %s alone when an invitation is sent",
    (pathname) => {
      mockPathname = pathname;
      render(<RealtimeListener />);
      MockEventSource.instances[0].emit("invite:sent", {});
      expect(mockRefresh).not.toHaveBeenCalled();
    },
  );
});

// A role changed under the person: the page's controls follow the role the
// server answers now, so the page is rendered again, wherever it is.
describe("RealtimeListener - membership:changed", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    MockEventSource.instances = [];
    mockActiveOrgId = "org_1";
  });

  it.each(["/acme/web/members", "/acme/settings", "/account/settings"])(
    "refreshes %s and moves nowhere",
    (pathname) => {
      mockPathname = pathname;
      render(<RealtimeListener />);
      MockEventSource.instances[0].emit("membership:changed", {
        organizationId: "org_1",
        projectId: "project_1",
      });
      expect(mockRefresh).toHaveBeenCalledTimes(1);
      expect(mockReplace).not.toHaveBeenCalled();
    },
  );
});

// Ingest's nudge: a page reading the project's spans is drawn again — the
// trace page for its own run alone, the Traces list counting instead unless
// it is empty — and nothing else moves.
describe("RealtimeListener - spans:ingested", () => {
  const nudge = (projectId: string, runs: string[] = ["run_1"], more = false) =>
    MockEventSource.instances[0].emit("spans:ingested", {
      organizationId: "org_1",
      projectId,
      runs,
      more,
    });

  beforeEach(() => {
    vi.clearAllMocks();
    MockEventSource.instances = [];
    mockActiveOrgId = "org_1";
    mockProjects = [
      { id: "project_1", slug: "web" },
      { id: "project_10", slug: "web-2" },
    ];
  });

  afterEach(() => {
    for (const project of ["project_1", "project_10"]) {
      listDrawn(project, { rows: [], empty: false, first: true });
    }
  });

  it.each(["/acme/web/sessions", "/acme/web/sessions/s_1", "/acme/web/users"])(
    "draws %s again for the project on screen",
    (pathname) => {
      mockPathname = pathname;
      render(<RealtimeListener />);
      nudge("project_1");
      expect(mockRefresh).toHaveBeenCalledTimes(1);
      expect(mockReplace).not.toHaveBeenCalled();
    },
  );

  it("counts the Traces list's new traces for its bar rather than drawing the list", () => {
    listDrawn("project_1", { rows: ["r1"], empty: false, first: true });
    mockPathname = "/acme/web/traces";
    render(<RealtimeListener />);
    nudge("project_1", ["run_1", "run_2"]);
    nudge("project_1", ["run_2", "r1"], true);
    expect(mockRefresh).not.toHaveBeenCalled();
    expect(newTraces("project_1"), "r1 is on the list already").toEqual({ count: 2, more: true });
  });

  // Nothing on an empty list can move, so its first trace shows itself —
  // under the hold and only in view, as the other pages are drawn.
  it("draws an empty Traces list rather than counting for it", () => {
    listDrawn("project_1", { rows: [], empty: true, first: true });
    mockPathname = "/acme/web/traces";
    render(<RealtimeListener />);
    nudge("project_1", ["run_1"]);
    expect(mockRefresh).toHaveBeenCalledTimes(1);
    expect(newTraces("project_1")).toEqual({ count: 0, more: false });
  });

  it("leaves a page of another project, and a page reading no spans, alone", () => {
    listDrawn("project_1", { rows: ["r1"], empty: false, first: true });
    mockPathname = "/acme/web/traces";
    render(<RealtimeListener />);
    nudge("project_10");
    expect(mockRefresh).not.toHaveBeenCalled();
    expect(newTraces("project_10")).toEqual({ count: 0, more: false });
    expect(newTraces("project_1")).toEqual({ count: 0, more: false });

    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);
    MockEventSource.instances[1].emit("spans:ingested", {
      organizationId: "org_1",
      projectId: "project_1",
      runs: ["run_1"],
      more: false,
    });
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("draws a trace's page for its own run, or when the batch held more than the nudge names", () => {
    mockPathname = "/acme/web/traces/run_7";
    render(<RealtimeListener />);
    nudge("project_1", ["run_1", "run_2"]);
    expect(mockRefresh).not.toHaveBeenCalled();
    nudge("project_1", ["run_1", "run_7"]);
    expect(mockRefresh).toHaveBeenCalledTimes(1);
  });

  it("draws a trace's page when its run may be among the ones the nudge left out", () => {
    mockPathname = "/acme/web/traces/run_7";
    render(<RealtimeListener />);
    nudge("project_1", ["run_1"], true);
    expect(mockRefresh).toHaveBeenCalledTimes(1);
  });

  it("holds between redraws for as long as the layout says", () => {
    vi.useFakeTimers();
    try {
      mockPathname = "/acme/web/sessions";
      render(<RealtimeListener liveHoldMs={100} />);
      nudge("project_1");
      nudge("project_1");
      nudge("project_1");
      expect(mockRefresh).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(100);
      expect(mockRefresh).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("leaves the download's route alone", () => {
    mockPathname = "/acme/web/traces/export";
    render(<RealtimeListener />);
    nudge("project_1", ["run_1"], true);
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  // jsdom pretends to be visual, so a hidden tab has to be said outright.
  it("draws a hidden tab once when it comes back into view, whatever landed meanwhile", () => {
    const visibility = vi.spyOn(document, "visibilityState", "get");
    try {
      visibility.mockReturnValue("hidden");
      mockPathname = "/acme/web/sessions";
      render(<RealtimeListener liveHoldMs={100} />);
      nudge("project_1");
      nudge("project_1");
      expect(mockRefresh).not.toHaveBeenCalled();

      visibility.mockReturnValue("visible");
      document.dispatchEvent(new Event("visibilitychange"));
      expect(mockRefresh).toHaveBeenCalledTimes(1);

      document.dispatchEvent(new Event("visibilitychange"));
      expect(mockRefresh, "nothing landed since").toHaveBeenCalledTimes(1);
    } finally {
      visibility.mockRestore();
    }
  });
});
