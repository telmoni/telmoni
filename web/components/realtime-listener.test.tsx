// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { RealtimeListener } from "./realtime-listener";

const mockReplace = vi.fn();
const mockRefresh = vi.fn();
let mockPathname = "/project_1/members";
let mockActiveOrgId: string | null = "org_1";

vi.mock("next/navigation", () => ({
  useRouter: () => ({
    replace: mockReplace,
    refresh: mockRefresh,
  }),
  usePathname: () => mockPathname,
}));

vi.mock("@/lib/store", () => ({
  useIncomingInvites: () => [],
  useOrganizations: () => [{ organizationId: "org_1" }],
  useActiveOrganizationId: () => mockActiveOrgId,
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
  });

  it("ejects the user to /console when removed from their active project subpath", () => {
    mockPathname = "/project_1/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).toHaveBeenCalledWith("/console");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("ejects the user to /console when removed from their active project root (/project_1)", () => {
    mockPathname = "/project_1";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).toHaveBeenCalledWith("/console");
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("does not false-positive match project ID prefix (/project_10 when removed from project_1)", () => {
    mockPathname = "/project_10/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emit("membership:removed", {
      organizationId: "org_1",
      projectId: "project_1",
    });

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).toHaveBeenCalled();
  });

  it("does not eject from /organization when only removed from a project in that org", () => {
    mockPathname = "/organization/projects";
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
    mockPathname = "/organization/members";
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
    mockPathname = "/organization/members";
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
    mockPathname = "/project_1/members";
    render(<RealtimeListener />);

    const es = MockEventSource.instances[0];
    es.emitRaw("membership:removed", "not-a-json");

    expect(mockReplace).not.toHaveBeenCalled();
    expect(mockRefresh).not.toHaveBeenCalled();
  });
});
