// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { RealtimeListener } from "@/components/realtime-listener";
import { PEEK_NUDGED } from "@/lib/traces";

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

// One router and one of each action across renders, as Next and the store
// hand them out, so a render alone never opens another stream.
const ROUTER = { replace: mockReplace, refresh: mockRefresh };
const NO_INVITES: never[] = [];
const mockAddInvite = vi.fn();
const mockRemoveInvite = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ROUTER,
  usePathname: () => mockPathname,
}));

vi.mock("@/lib/store", () => ({
  useIncomingInvites: () => NO_INVITES,
  useOrganizations: () => ORGANIZATIONS,
  useProjects: () => mockProjects,
  useActiveOrganizationId: () => mockActiveOrgId,
  useActiveOrganization: () =>
    ORGANIZATIONS.find((o) => o.organizationId === mockActiveOrgId) ?? null,
  useAddIncomingInvite: () => mockAddInvite,
  useRemoveIncomingInvite: () => mockRemoveInvite,
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

  it("listens on the console's versioned events lane", () => {
    mockPathname = "/acme/web";
    render(<RealtimeListener />);

    expect(MockEventSource.instances[0]!.url).toBe("/api/v1/events");
  });

  it("ejects the user to /console when removed from their active project subpath", () => {
    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0]!;
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

    const es = MockEventSource.instances[0]!;
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

    const es = MockEventSource.instances[0]!;
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

    const es = MockEventSource.instances[0]!;
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

    const es = MockEventSource.instances[0]!;
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

    const es = MockEventSource.instances[0]!;
    es.emit("membership:removed", {
      organizationId: "org_1",
    });

    expect(mockReplace).toHaveBeenCalledWith("/console");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("does not eject from /settings/account when removed from an organization", () => {
    mockPathname = "/settings/account";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0]!;
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

    const es = MockEventSource.instances[0]!;
    es.emit("membership:removed", {
      organizationId: "org_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("safely ignores malformed JSON or unparseable event data", () => {
    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0]!;
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

    MockEventSource.instances[0]!.emit("ownership:changed", {
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

    MockEventSource.instances[0]!.emit("ownership:changed", {
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

    MockEventSource.instances[0]!.emit("ownership:changed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("refreshes in place when another project moved, or the organization changed hands", () => {
    mockPathname = "/acme/web-2";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0]!;
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
      MockEventSource.instances[0]!.emit("invite:sent", {});
      expect(mockRefresh).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["/acme", "/acme/settings", "/acme/web", "/acme/web/api-keys", "/settings/account"])(
    "leaves %s alone when an invitation is sent",
    (pathname) => {
      mockPathname = pathname;
      render(<RealtimeListener />);
      MockEventSource.instances[0]!.emit("invite:sent", {});
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

  it.each(["/acme/web/members", "/acme/settings", "/settings/account"])(
    "refreshes %s and moves nowhere",
    (pathname) => {
      mockPathname = pathname;
      render(<RealtimeListener />);
      MockEventSource.instances[0]!.emit("membership:changed", {
        organizationId: "org_1",
        projectId: "project_1",
      });
      expect(mockRefresh).toHaveBeenCalledTimes(1);
      expect(mockReplace).not.toHaveBeenCalled();
    },
  );
});

// Ingest's nudge: a page reading the project's spans is drawn again — the
// trace page for its own trace alone, the Traces list counting instead unless
// it is empty — and nothing else moves.
describe("RealtimeListener - spans:ingested", () => {
  const nudge = (projectId: string, traces: string[] = ["trace_1"], more = false) =>
    MockEventSource.instances[0]!.emit("spans:ingested", {
      organizationId: "org_1",
      projectId,
      traces,
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

  it("draws the Traces list as its traces land, held as every page is", () => {
    vi.useFakeTimers();
    try {
      mockPathname = "/acme/web/traces";
      render(<RealtimeListener liveHoldMs={100} />);
      nudge("project_1", ["trace_1", "trace_2"]);
      nudge("project_1", ["trace_3"], true);
      expect(mockRefresh).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(100);
      expect(mockRefresh).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("draws the Agents page, held as a trace's page is", () => {
    vi.useFakeTimers();
    try {
      mockPathname = "/acme/web";
      render(<RealtimeListener liveHoldMs={100} />);
      nudge("project_1", ["trace_1"]);
      nudge("project_1", ["trace_2"]);
      expect(mockRefresh).toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(100);
      expect(mockRefresh).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("leaves a page of another project, and a page reading no spans, alone", () => {
    mockPathname = "/acme/web/traces";
    render(<RealtimeListener />);
    nudge("project_10");
    expect(mockRefresh).not.toHaveBeenCalled();

    mockPathname = "/acme/web/members";
    render(<RealtimeListener />);
    MockEventSource.instances[1]!.emit("spans:ingested", {
      organizationId: "org_1",
      projectId: "project_1",
      traces: ["trace_1"],
      more: false,
    });
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("draws a trace's page for its own trace, or when the batch held more than the nudge names", () => {
    mockPathname = "/acme/web/traces/trace_7";
    render(<RealtimeListener />);
    nudge("project_1", ["trace_1", "trace_2"]);
    expect(mockRefresh).not.toHaveBeenCalled();
    nudge("project_1", ["trace_1", "trace_7"]);
    expect(mockRefresh).toHaveBeenCalledTimes(1);
  });

  it("draws a trace's page when its trace may be among the ones the nudge left out", () => {
    mockPathname = "/acme/web/traces/trace_7";
    render(<RealtimeListener />);
    nudge("project_1", ["trace_1"], true);
    expect(mockRefresh).toHaveBeenCalledTimes(1);
  });

  it("holds between redraws for as long as the layout says", () => {
    vi.useFakeTimers();
    try {
      mockPathname = "/acme/web/traces/trace_1";
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

  // The list's peek is read again when its own trace lands, held as a page is.
  it("tells the list's peek when its own trace lands", () => {
    vi.useFakeTimers();
    const landed = vi.fn();
    window.addEventListener(PEEK_NUDGED, landed);
    try {
      window.history.replaceState(null, "", "/acme/web/traces?peek=trace_7");
      mockPathname = "/acme/web/traces";
      render(<RealtimeListener liveHoldMs={100} />);
      nudge("project_1", ["trace_1"]);
      expect(landed).not.toHaveBeenCalled();
      nudge("project_1", ["trace_7"]);
      expect(landed).toHaveBeenCalledTimes(1);
      nudge("project_1", ["trace_1"], true);
      expect(landed, "held, as a page is").toHaveBeenCalledTimes(1);
      vi.advanceTimersByTime(100);
      expect(landed).toHaveBeenCalledTimes(2);
    } finally {
      window.removeEventListener(PEEK_NUDGED, landed);
      window.history.replaceState(null, "", "/");
      vi.useRealTimers();
    }
  });

  it("leaves a download's route alone", () => {
    mockPathname = "/acme/web/traces/export";
    render(<RealtimeListener />);
    nudge("project_1", ["trace_1"], true);
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  // jsdom pretends to be visual, so a hidden tab has to be said outright.
  it("draws a hidden tab once when it comes back into view, whatever landed meanwhile", () => {
    const visibility = vi.spyOn(document, "visibilityState", "get");
    try {
      visibility.mockReturnValue("hidden");
      mockPathname = "/acme/web/traces/trace_1";
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

describe("RealtimeListener - what the person reads", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    MockEventSource.instances = [];
    mockActiveOrgId = "org_1";
    mockPathname = "/acme/web/traces";
  });

  // The stream passes a project's nudges on to its readers as it found them
  // when it opened, so a change of seat or role needs a stream of its own.
  it("opens its stream again when the projects the person reads change, and only then", () => {
    const { rerender } = render(<RealtimeListener readingKey="org_1:member" />);
    rerender(<RealtimeListener readingKey="org_1:member" />);
    expect(MockEventSource.instances).toHaveLength(1);

    rerender(<RealtimeListener readingKey={"org_1/project_1\norg_1:member"} />);
    expect(MockEventSource.instances).toHaveLength(2);
    expect(MockEventSource.instances[0]!.readyState).toBe(2);
  });
});
