// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let mockSearch = new URLSearchParams();
vi.mock("next/navigation", () => ({
  useSearchParams: () => mockSearch,
  usePathname: () => "/acme/web/traces",
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn(), prefetch: vi.fn() }),
}));

const uncover = vi.fn();
const cover = vi.fn(() => uncover);
let mockShell: HTMLElement | null = null;
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: mockShell, cover }),
}));

import { TracePeek } from "@/components/traces/peek";
import { TooltipProvider } from "@/components/ui/tooltip";
import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";

const TRACE_A = "5d4f5c6800004000800000000000000a";
const TRACE_B = "5d4f5c6800004000800000000000000b";

function trace(traceId: string, tags: Record<string, string> = {}) {
  return {
    trace_id: traceId,
    truncated: false,
    spend: null,
    content: [],
    spans: [
      {
        span_id: "0000000000000001",
        parent_span_id: "",
        trace_id: traceId,
        agent: `agent-${traceId.slice(-1)}`,
        kind: "agent",
        name: "nightly-report",
        started_at: "2026-10-11T03:00:00.000Z",
        ended_at: "2026-10-11T03:00:02.000Z",
        status: "ok",
        error_type: "",
        model: "",
        provider: "",
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        cost: null,
        time_to_first_token: null,
        user_id: "",
        session_id: "",
        environment: "",
        release: "",
        tags,
        sample_rate: null,
        attributes: {},
        units: 1,
        received_at: "2026-10-11T03:00:02.000Z",
      },
    ],
  };
}

/** The server's answers, by the trace each read names. */
let answers: Record<string, () => Response>;
const fetchMock = vi.fn(async (url: string) => {
  const traceId = new URL(url, "http://localhost").pathname.split("/").pop()!;
  return answers[traceId]?.() ?? new Response("{}", { status: 404 });
});

function open(traceId: string) {
  mockSearch = new URLSearchParams(`days=30&peek=${traceId}`);
  window.history.replaceState(null, "", `/acme/web/traces?days=30&peek=${traceId}`);
}

function peek() {
  return render(
    <TooltipProvider>
      <TracePeek
        organization="acme"
        slug="web"
        projectId="project_1"
        traces={[TRACE_A, TRACE_B]}
        me="user_1"
        administers={false}
      />
    </TooltipProvider>,
  );
}

const peeked = () => new URL(window.location.href).searchParams.get("peek");

describe("TracePeek", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockShell = document.body;
    window.localStorage.removeItem(TRACE_PREFS_KEY);
    answers = {
      [TRACE_A]: () => Response.json(trace(TRACE_A)),
      [TRACE_B]: () => Response.json(trace(TRACE_B)),
    };
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.replaceState(null, "", "/");
  });

  it("stands shut while the URL names no trace", () => {
    mockSearch = new URLSearchParams("days=30");
    peek();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("opens beside the list, the page left in reach, and reads the trace's spans by the project's id", async () => {
    open(TRACE_A);
    peek();
    const panel = screen.getByRole("dialog");
    expect(panel).toHaveAttribute("aria-modal", "false");
    expect(document.activeElement).toBe(panel);
    expect(await within(panel).findByRole("tree", { name: "Spans" })).toBeInTheDocument();
    expect(within(panel).getByRole("heading", { level: 2 })).toHaveTextContent("agent-a");
    expect(fetchMock).toHaveBeenCalledWith(`/api/v1/traces/${TRACE_A}?project=project_1`, expect.anything());
    expect(cover).not.toHaveBeenCalled();
  });

  it("steps to the next trace in place, and holds still at the list's start", async () => {
    open(TRACE_A);
    peek();
    const panel = screen.getByRole("dialog");
    await within(panel).findByRole("tree");
    const previous = within(panel).getByRole("button", { name: "Previous trace" });
    expect(previous).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(previous);
    expect(peeked()).toBe(TRACE_A);

    const length = window.history.length;
    fireEvent.click(within(panel).getByRole("button", { name: "Next trace" }));
    expect(peeked()).toBe(TRACE_B);
    expect(window.history.length, "a step is the same entry").toBe(length);
    expect(new URLSearchParams(window.location.search).get("days")).toBe("30");
  });

  it("fills the content area at a press, the list then out of reach", async () => {
    open(TRACE_A);
    peek();
    const panel = screen.getByRole("dialog");
    fireEvent.click(within(panel).getByRole("button", { name: "Full size" }));
    expect(panel).toHaveAttribute("data-mode", "full");
    expect(cover).toHaveBeenCalledTimes(1);
    expect(uncover).not.toHaveBeenCalled();
    expect(within(panel).getByRole("button", { name: "Exit full size" })).toBeInTheDocument();
  });

  it("shuts on Escape from inside, rewriting a link's URL without the trace", async () => {
    open(TRACE_A);
    peek();
    const panel = screen.getByRole("dialog");
    await within(panel).findByRole("tree");
    fireEvent.keyDown(panel, { key: "Escape" });
    expect(peeked()).toBeNull();
    expect(window.location.search).toBe("?days=30");
  });

  it("says when the trace is not there, and when the session has ended, keeping the list's query", async () => {
    answers[TRACE_A] = () => new Response("{}", { status: 404 });
    open(TRACE_A);
    const { unmount } = peek();
    expect(await screen.findByText("No trace with this id in this project.")).toBeInTheDocument();
    unmount();

    answers[TRACE_A] = () => new Response("{}", { status: 401 });
    peek();
    const again = await screen.findByRole("link", { name: "Sign in again" });
    expect(again).toHaveAttribute(
      "href",
      `/auth/login?returnTo=${encodeURIComponent(`/acme/web/traces?days=30&peek=${TRACE_A}`)}`,
    );
  });

  it("shows the trace's tags, each a link to the traces that carry it", async () => {
    answers[TRACE_A] = () => Response.json(trace(TRACE_A, { team: "search" }));
    open(TRACE_A);
    peek();
    const panel = screen.getByRole("dialog");
    const tag = await within(panel).findByRole("link", { name: "team:search" });
    expect(tag.getAttribute("href")).toContain("tag=team%3Asearch");
  });

  it("offers a retry when the trace could not be read", async () => {
    answers[TRACE_A] = () => new Response("{}", { status: 503 });
    open(TRACE_A);
    peek();
    fireEvent.click(await screen.findByRole("button", { name: "Try again" }));
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});
