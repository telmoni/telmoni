// @vitest-environment jsdom
import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  CONSOLE_TRAIL_KEY,
  liveResources,
  resolveReturnUrl,
  resourceUrl,
  type Resource,
} from "./console-trail";
import {
  clearTrailForTest,
  readTrail,
  recordPath,
  useConsoleTrail,
  useRecordConsolePath,
} from "./use-console-trail";

const ORGANIZATION: Resource = { kind: "organization", organization: "acme" };
const PROJECT: Resource = { kind: "project", organization: "acme", project: "web" };
const LIVE = liveResources(
  [{ slug: "acme" }],
  [
    { slug: "web", organizationSlug: "acme" },
    { slug: "api", organizationSlug: "acme" },
  ],
);
const OVERVIEW = "/acme/web";
const CONNECTORS = `${OVERVIEW}/connectors`;
const ORG_MEMBERS = "/acme/members";

beforeEach(() => {
  window.sessionStorage.clear();
  clearTrailForTest();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("this tab's trail", () => {
  it("is empty until a page records one", () => {
    expect(readTrail()).toEqual([]);
  });

  it("is written through to storage and read back on a reload", () => {
    recordPath(CONNECTORS);
    recordPath(ORG_MEMBERS);
    expect(JSON.parse(window.sessionStorage.getItem(CONSOLE_TRAIL_KEY)!)).toEqual(
      [ORG_MEMBERS, CONNECTORS],
    );
    clearTrailForTest();
    expect(readTrail()).toEqual([ORG_MEMBERS, CONNECTORS]);
  });

  it("records nothing for a path it would refuse", () => {
    recordPath("/account/settings");
    expect(window.sessionStorage.getItem(CONSOLE_TRAIL_KEY)).toBeNull();
  });

  // Same reference, not merely equal: the snapshot contract the hook depends on.
  it("keeps the same trail when the page has not changed", () => {
    recordPath(CONNECTORS);
    const first = readTrail();
    recordPath(CONNECTORS);
    expect(readTrail()).toBe(first);
  });

  it("still navigates for the life of a tab that has storage denied", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    recordPath(CONNECTORS);
    expect(readTrail()).toEqual([CONNECTORS]);
  });

  it("ignores stored entries it would not have written", () => {
    window.sessionStorage.setItem(
      CONSOLE_TRAIL_KEY,
      JSON.stringify(["//evil.example.com", CONNECTORS]),
    );
    expect(readTrail()).toEqual([CONNECTORS]);
  });
});

describe("the console across a visit", () => {
  /** The layout's recorder and a reader, standing on one page. */
  function standAt(pathname: string) {
    return renderHook(() => {
      useRecordConsolePath(pathname);
      return useConsoleTrail();
    });
  }

  it("records where you stood, then hands it to the back arrow", () => {
    standAt(CONNECTORS).unmount();
    const trail = standAt("/account/settings").result.current;
    expect(resolveReturnUrl(trail, LIVE, OVERVIEW)).toBe(CONNECTORS);
  });

  it("holds the page while you move around Account", () => {
    standAt(CONNECTORS).unmount();
    const account = standAt("/account/settings");
    account.rerender();
    expect(resolveReturnUrl(account.result.current, LIVE, OVERVIEW)).toBe(CONNECTORS);
  });

  // Connectors, off to the organization, and back through the selector.
  it("sends a resource switch back to where you were in it", () => {
    standAt(CONNECTORS).unmount();
    const org = standAt(ORG_MEMBERS);
    expect(resourceUrl(org.result.current, PROJECT)).toBe(CONNECTORS);
    org.unmount();
    const back = standAt(CONNECTORS);
    expect(resourceUrl(back.result.current, ORGANIZATION)).toBe(ORG_MEMBERS);
  });

  it("offers the fallback to a tab that opened on an account page", () => {
    const trail = standAt("/account/notifications").result.current;
    expect(resolveReturnUrl(trail, LIVE, OVERVIEW)).toBe(OVERVIEW);
  });

  // No frame on the fallback: the trail is read on the first render rather
  // than corrected by an effect afterwards.
  it("reads the trail on its very first render", () => {
    recordPath(CONNECTORS);
    const seen: string[] = [];
    renderHook(() => {
      seen.push(resolveReturnUrl(useConsoleTrail(), LIVE, OVERVIEW));
    });
    expect(seen[0]).toBe(CONNECTORS);
  });

  // A reader that is not the recorder still sees the write: the selector sits
  // in the rail, the recorder in the layout.
  it("updates every reader when the recorder writes", () => {
    const reader = renderHook(() => useConsoleTrail());
    expect(reader.result.current).toEqual([]);
    const recorder = standAt(CONNECTORS);
    expect(reader.result.current).toEqual([CONNECTORS]);
    recorder.unmount();
    reader.unmount();
  });
});
