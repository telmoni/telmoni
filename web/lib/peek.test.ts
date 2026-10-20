// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  PEEKED,
  PEEK_WIDTH_MIN,
  closePeek,
  draggedWidth,
  followEntry,
  keepRead,
  openPeek,
  peekUrl,
  peekedRun,
  pushedEntry,
  pushedHere,
  shownRead,
  type KeptRead,
} from "./peek";

const RUN_A = "5d4f5c68-0000-4000-8000-000000000001";
const RUN_B = "5d4f5c68-0000-4000-8000-000000000002";
const peeked = () => new URL(window.location.href).searchParams.get("peek");
/** The traversal a shut peek started, landed: its own listener runs first. */
const landed = () => new Promise<void>((resolve) => window.addEventListener("popstate", () => resolve(), { once: true }));

describe("the peek in the browser's history", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/acme/web/traces?status=error&days=30");
  });

  it("reads a run's UUID out of the URL, any UUID's shape, and nothing else", () => {
    expect(peekedRun(RUN_A)).toBe(RUN_A);
    expect(peekedRun("11111111-1111-1111-1111-111111111111")).toBe(
      "11111111-1111-1111-1111-111111111111",
    );
    expect(peekedRun("not-a-run")).toBeNull();
    expect(peekedRun(null)).toBeNull();
  });

  it("keeps the list's own query beside the run", () => {
    expect(peekUrl(RUN_A)).toBe(`/acme/web/traces?status=error&days=30&peek=${RUN_A}`);
    expect(peekUrl(null)).toBe("/acme/web/traces?status=error&days=30");
  });

  it("pushes an entry of its own from a shut peek, and steps in place while open", () => {
    const length = window.history.length;
    openPeek(RUN_A);
    expect(window.history.length).toBe(length + 1);
    expect(pushedHere()).toBe(true);
    expect(peeked()).toBe(RUN_A);

    openPeek(RUN_B);
    expect(window.history.length, "a step is the same entry").toBe(length + 1);
    expect(pushedHere()).toBe(true);
    expect(peeked()).toBe(RUN_B);
  });

  it("opened from a link, steps in place without claiming the entry", () => {
    window.history.replaceState(null, "", `/acme/web/traces?peek=${RUN_A}`);
    openPeek(RUN_B);
    expect(pushedHere()).toBe(false);
    expect(peeked()).toBe(RUN_B);
  });

  it("shuts by going back over the entry it pushed, or rewrites a link's URL without it", async () => {
    openPeek(RUN_A);
    const length = window.history.length;
    closePeek(true);
    await landed();
    expect(window.location.search).toBe("?status=error&days=30");
    expect(window.history.state, "the list's entry as it was").toBeNull();

    window.history.replaceState(null, "", `/acme/web/traces?days=30&peek=${RUN_A}`);
    closePeek(false);
    expect(window.location.search).toBe("?days=30");
    expect(window.history.length).toBe(length);
  });

  // Else the list's entry still names the peek's first layout, which the next peek reads as a link's.
  it("gives the entry it goes back to the address the list now stands at", async () => {
    window.history.replaceState(null, "", "/acme/web/sessions?days=30&view=v1");
    openPeek(RUN_A);
    const length = window.history.length;
    // A layout chosen in the peek, its rewrite keeping the mark (`use-layout.ts`).
    window.history.replaceState({ [PEEKED]: true }, "", `/acme/web/sessions?days=30&view=v2&peek=${RUN_A}`);
    closePeek(true);
    await landed();
    expect(window.location.search).toBe("?days=30&view=v2");
    expect(window.history.length, "and no entry more").toBe(length);
    expect(window.history.state, "a fresh state, which Next's router follows").toEqual({});
  });

  it("rewrites no entry a traversal it did not start lands on, the same list's in another window", async () => {
    openPeek(RUN_A);
    const back = vi.spyOn(window.history, "back").mockImplementation(() => {});
    closePeek(true);
    back.mockRestore();
    window.history.pushState(null, "", "/acme/web/traces?days=1");
    window.history.pushState(null, "", "/acme/web/traces?days=2");
    window.history.back();
    await landed();
    expect(window.location.search).toBe("?days=1");
  });

  it("goes back no further when pressed again while the list is on its way", async () => {
    openPeek(RUN_A);
    closePeek(true);
    await landed();
    const back = vi.spyOn(window.history, "back");
    try {
      closePeek(true);
      expect(back).not.toHaveBeenCalled();
      expect(window.location.search).toBe("?status=error&days=30");
    } finally {
      back.mockRestore();
    }
  });

  it("leaves the list's entry as it was where only its spelling moved", async () => {
    window.history.replaceState(null, "", "/acme/web/traces?user=a@b.com&days=30");
    openPeek(RUN_A);
    closePeek(true);
    await landed();
    expect(window.location.search).toBe("?user=a@b.com&days=30");
    expect(window.history.state).toBeNull();
  });

  it("follows the entry it pushed as a step moves it to another run in place", () => {
    openPeek(RUN_A);
    const entry = pushedEntry();
    expect(entry).toBe(`/acme/web/traces?status=error&days=30&peek=${RUN_A}`);
    openPeek(RUN_B);
    expect(followEntry(entry)).toBe(`/acme/web/traces?status=error&days=30&peek=${RUN_B}`);
  });

  it("writes its mark back where a router wrote over the entry, keeping the router's own", () => {
    openPeek(RUN_A);
    const entry = pushedEntry();
    // A refresh: the router's own state over the same entry, at the same address.
    window.history.replaceState({ __NA: true, tree: "t" }, "");
    expect(pushedHere()).toBe(false);
    expect(followEntry(entry)).toBe(entry);
    expect(window.history.state).toEqual({ __NA: true, tree: "t", [PEEKED]: true });
  });

  // The list moved to another window with the peek kept: a new entry of the
  // router's, which shutting the peek must not go back over.
  it("lets go of an entry the list moved on to, and leaves it unmarked", () => {
    openPeek(RUN_A);
    const entry = pushedEntry();
    window.history.pushState({ __NA: true }, "", `/acme/web/traces?days=7&peek=${RUN_A}`);
    expect(followEntry(entry)).toBeNull();
    expect(pushedHere()).toBe(false);
    expect(followEntry(null)).toBeNull();
  });

  // Else Close, rewriting in place, would leave the duplicate it goes back to spare.
  it("takes its entry back where Back returns to it from the list moved on", async () => {
    openPeek(RUN_A);
    const entry = pushedEntry();
    window.history.pushState({ __NA: true }, "", `/acme/web/traces?days=7&peek=${RUN_A}`);
    expect(followEntry(entry)).toBeNull();
    window.history.back();
    await landed();
    expect(followEntry(null)).toBe(entry);
  });
});

describe("the panel's edge dragged", () => {
  it("widens leftwards and narrows rightwards, between its least and the area it stands in", () => {
    expect(draggedWidth(640, 500, 400, 1000)).toBe(740);
    expect(draggedWidth(640, 500, 560, 1000)).toBe(580);
    expect(draggedWidth(640, 500, 900, 1000)).toBe(PEEK_WIDTH_MIN);
    expect(draggedWidth(640, 500, -200, 1000)).toBe(1000);
  });

  it("never stands wider than an area narrower than its least", () => {
    expect(draggedWidth(400, 500, 900, 420)).toBe(420);
  });
});

describe("what the panel shows of a trace's reads", () => {
  const A = "/api/v1/traces/a?project=p";
  const B = "/api/v1/traces/b?project=p";
  const kept = (url: string, version: number, read: KeptRead<string>["read"]): KeptRead<string> => ({
    url,
    version,
    read,
  });

  it("shows the read asked for, and nothing before any came", () => {
    expect(shownRead(null, A, 0)).toEqual({ read: null, stale: false });
    expect(shownRead(kept(A, 0, { kind: "ok", trace: "a" }), A, 0)).toEqual({
      read: { kind: "ok", trace: "a" },
      stale: false,
    });
  });

  it("keeps the last run's trace, marked stale, while the next is on its way", () => {
    expect(shownRead(kept(A, 0, { kind: "ok", trace: "a" }), B, 0)).toEqual({
      read: { kind: "ok", trace: "a" },
      stale: true,
    });
    expect(shownRead(kept(A, 0, { kind: "missing" }), B, 0)).toEqual({ read: null, stale: false });
  });

  it("keeps a trace on show while it is read again, and says a failure is being tried again", () => {
    expect(shownRead(kept(A, 0, { kind: "ok", trace: "a" }), A, 1).read).toEqual({
      kind: "ok",
      trace: "a",
    });
    expect(shownRead(kept(A, 0, { kind: "unavailable" }), A, 1)).toEqual({ read: null, stale: false });
  });

  it("keeps the trace on show when a read for its spans landing fails", () => {
    const good = kept(A, 0, { kind: "ok", trace: "a" });
    expect(keepRead(good, kept(A, 1, { kind: "unavailable" }))).toEqual(kept(A, 1, { kind: "ok", trace: "a" }));
    expect(keepRead(good, kept(A, 1, { kind: "ok", trace: "a2" }))).toEqual(kept(A, 1, { kind: "ok", trace: "a2" }));
  });

  it("takes any answer for another run, and a retry's after a failure", () => {
    const good = kept(A, 0, { kind: "ok", trace: "a" });
    expect(keepRead(good, kept(B, 0, { kind: "missing" }))).toEqual(kept(B, 0, { kind: "missing" }));
    const failed = kept(A, 0, { kind: "unavailable" });
    expect(keepRead(failed, kept(A, 1, { kind: "forbidden" }))).toEqual(kept(A, 1, { kind: "forbidden" }));
  });
});
