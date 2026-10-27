// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SessionHeartbeat } from "@/components/session-heartbeat";

const NOW = new Date("2026-10-15T12:00:00.000Z").getTime();

/** The heartbeat's answers, in order; the last repeats. */
let answers: Array<() => Promise<Response>>;
const fetchMock = vi.fn((_url: string, init?: RequestInit) => {
  const next = answers.length > 1 ? answers.shift()! : answers[0]!;
  void init;
  return next();
});

const renewed = () =>
  Promise.resolve(Response.json({ ok: true, expiresAt: NOW + 15 * 60_000 }));
const unanswered = () =>
  Promise.resolve(
    Response.json({ type: "about:blank", title: "unavailable", status: 503 }, { status: 503 }),
  );

// A renewal nobody answered may have been made all the same, and auth takes
// the spent token back for thirty seconds alone: the heartbeat asks again
// inside them, however it went unanswered.
describe("SessionHeartbeat - a renewal nobody answered", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    fetchMock.mockClear();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("asks again five seconds after a 503, and then keeps the usual pace", async () => {
    answers = [unanswered, renewed];
    render(<SessionHeartbeat expiresAt={NOW + 60_000} />);
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(4_999);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    // Renewed for fifteen minutes: the next ping waits for the last ninety
    // seconds of them.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("asks again five seconds after a ping cut off, not at the usual pace", async () => {
    answers = [
      () =>
        new Promise<Response>((_, reject) => {
          const signal = fetchMock.mock.calls.at(-1)![1]!.signal!;
          signal.addEventListener("abort", () => reject(new DOMException("cut off", "AbortError")));
        }),
      renewed,
    ];
    render(<SessionHeartbeat expiresAt={NOW + 60_000} />);
    await vi.advanceTimersByTimeAsync(15_000);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("asks again in five seconds even when the ping came early, from focus", async () => {
    answers = [unanswered, renewed];
    // Two minutes left: nothing pings on mount, focus does, and its retry is
    // not held to the schedule's mark ninety seconds before expiry.
    render(<SessionHeartbeat expiresAt={NOW + 119_000} />);
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(5_000);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("goes back to the usual pace once the grace is spent on quick asks", async () => {
    answers = [unanswered];
    render(<SessionHeartbeat expiresAt={NOW + 60_000} />);
    await vi.advanceTimersByTimeAsync(0);
    // The first ask and four quick ones.
    await vi.advanceTimersByTimeAsync(4 * 5_000);
    expect(fetchMock).toHaveBeenCalledTimes(5);
    await vi.advanceTimersByTimeAsync(29_999);
    expect(fetchMock).toHaveBeenCalledTimes(5);
    await vi.advanceTimersByTimeAsync(1);
    expect(fetchMock).toHaveBeenCalledTimes(6);
  });
});
