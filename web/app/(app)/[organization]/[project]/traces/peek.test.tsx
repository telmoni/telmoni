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

const RUN_A = "5d4f5c68-0000-4000-8000-00000000000a";
const RUN_B = "5d4f5c68-0000-4000-8000-00000000000b";

function trace(run: string) {
  return {
    run_id: run,
    truncated: false,
    spend: null,
    spans: [
      {
        span_id: "0000000000000001",
        parent_span_id: "",
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
        agent: `agent-${run.slice(-1)}`,
        kind: "run",
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
        user_id: "",
        session_id: "",
        environment: "",
        release: "",
        tags: {},
        sample_rate: null,
        attributes: {},
        units: 1,
        received_at: "2026-10-11T03:00:02.000Z",
      },
    ],
  };
}

/** The server's answers, by the run each read names. */
let answers: Record<string, () => Response>;
const fetchMock = vi.fn(async (url: string) => {
  const run = new URL(url, "http://localhost").pathname.split("/").pop()!;
  return answers[run]?.() ?? new Response("{}", { status: 404 });
});

function open(run: string) {
  mockSearch = new URLSearchParams(`days=30&peek=${run}`);
  window.history.replaceState(null, "", `/acme/web/traces?days=30&peek=${run}`);
}

function peek() {
  return render(
    <TooltipProvider>
      <TracePeek organization="acme" slug="web" projectId="project_1" runs={[RUN_A, RUN_B]} />
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
      [RUN_A]: () => Response.json(trace(RUN_A)),
      [RUN_B]: () => Response.json(trace(RUN_B)),
    };
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.replaceState(null, "", "/");
  });

  it("stands shut while the URL names no run", () => {
    mockSearch = new URLSearchParams("days=30");
    peek();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("opens beside the list, the page left in reach, and reads the run's spans by the project's id", async () => {
    open(RUN_A);
    peek();
    const panel = screen.getByRole("dialog");
    expect(panel).toHaveAttribute("aria-modal", "false");
    expect(document.activeElement).toBe(panel);
    expect(await within(panel).findByRole("tree", { name: "Spans" })).toBeInTheDocument();
    expect(within(panel).getByRole("heading", { level: 2 })).toHaveTextContent("agent-a");
    expect(fetchMock).toHaveBeenCalledWith(`/api/v1/traces/${RUN_A}?project=project_1`, expect.anything());
    expect(cover).not.toHaveBeenCalled();
  });

  it("steps to the next run in place, and holds still at the list's start", async () => {
    open(RUN_A);
    peek();
    const panel = screen.getByRole("dialog");
    await within(panel).findByRole("tree");
    const previous = within(panel).getByRole("button", { name: "Previous trace" });
    expect(previous).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(previous);
    expect(peeked()).toBe(RUN_A);

    const length = window.history.length;
    fireEvent.click(within(panel).getByRole("button", { name: "Next trace" }));
    expect(peeked()).toBe(RUN_B);
    expect(window.history.length, "a step is the same entry").toBe(length);
    expect(new URLSearchParams(window.location.search).get("days")).toBe("30");
  });

  it("fills the content area at a press, the list then out of reach", async () => {
    open(RUN_A);
    peek();
    const panel = screen.getByRole("dialog");
    fireEvent.click(within(panel).getByRole("button", { name: "Full size" }));
    expect(panel).toHaveAttribute("data-mode", "full");
    expect(cover).toHaveBeenCalledTimes(1);
    expect(uncover).not.toHaveBeenCalled();
    expect(within(panel).getByRole("button", { name: "Exit full size" })).toBeInTheDocument();
  });

  it("shuts on Escape from inside, rewriting a link's URL without the run", async () => {
    open(RUN_A);
    peek();
    const panel = screen.getByRole("dialog");
    await within(panel).findByRole("tree");
    fireEvent.keyDown(panel, { key: "Escape" });
    expect(peeked()).toBeNull();
    expect(window.location.search).toBe("?days=30");
  });

  it("says when the run is not there, and when the session has ended, keeping the list's query", async () => {
    answers[RUN_A] = () => new Response("{}", { status: 404 });
    open(RUN_A);
    const { unmount } = peek();
    expect(await screen.findByText("No trace with this id in this project.")).toBeInTheDocument();
    unmount();

    answers[RUN_A] = () => new Response("{}", { status: 401 });
    peek();
    const again = await screen.findByRole("link", { name: "Sign in again" });
    expect(again).toHaveAttribute(
      "href",
      `/auth/login?returnTo=${encodeURIComponent(`/acme/web/traces?days=30&peek=${RUN_A}`)}`,
    );
  });

  it("offers a retry when the trace could not be read", async () => {
    answers[RUN_A] = () => new Response("{}", { status: 503 });
    open(RUN_A);
    peek();
    fireEvent.click(await screen.findByRole("button", { name: "Try again" }));
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});
