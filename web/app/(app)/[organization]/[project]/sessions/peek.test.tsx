// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let mockSearch = new URLSearchParams();
vi.mock("next/navigation", () => ({
  useSearchParams: () => mockSearch,
  usePathname: () => "/acme/web/sessions",
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn(), prefetch: vi.fn() }),
}));

const uncover = vi.fn();
const cover = vi.fn(() => uncover);
let mockShell: HTMLElement | null = null;
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: mockShell, cover }),
}));

vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({
  saveTraceViewAction: vi.fn(),
  updateTraceViewAction: vi.fn(),
  deleteTraceViewAction: vi.fn(),
  setSessionLayoutAction: vi.fn(),
}));

import { SessionPeek } from "@/components/traces/session-peek";
import { DEFAULT_LAYOUT } from "@/lib/layout";
import { TRACE_PREFS_KEY } from "@/lib/trace-prefs";
import { setTracePrefs } from "@/lib/use-trace-prefs";
import { PEEK_NUDGED } from "@/lib/traces";

const A = "conv-1";
// An id of the application's choosing, with a slash and a space.
const B = "chat/42 b";
const HASHED = "9f86d081884c7d659a2feaa0c55ad015";
const RUN = "5d4f5c68-0000-4000-8000-000000000001";

function detail(id: string) {
  return {
    session_id: id,
    traces: [
      {
        run_id: RUN,
        agent: "support-bot",
        user_id: HASHED,
        session_id: id,
        ended: true,
        status: "error",
        error_type: "RuntimeError",
        started_at: "2026-10-11T03:00:00Z",
        ended_at: "2026-10-11T03:00:02Z",
        duration_ms: 2000,
        time_to_first_token_ms: null,
        spans: 4,
        steps: 3,
        model_calls: 1,
        tool_calls: 1,
        errors: 1,
        input_tokens: 1200,
        output_tokens: 300,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        models: ["claude-sonnet-5-5"],
        tools: ["search_docs"],
        environments: [],
        releases: [],
        pinned: [],
      },
    ],
    pinned: [],
    days: 30,
    limit: 100,
    more: false,
  };
}

/** The server's answers, by the session each read names. */
let answers: Record<string, () => Response>;
const fetchMock = vi.fn(async (url: string) => {
  const id = decodeURIComponent(new URL(url, "http://localhost").pathname.split("/").pop()!);
  return answers[id]?.() ?? new Response("{}", { status: 404 });
});

function open(id: string) {
  const query = new URLSearchParams({ days: "30", peek: id });
  mockSearch = query;
  window.history.replaceState(null, "", `/acme/web/sessions?${query}`);
}

function peek() {
  return render(
    <SessionPeek
      organization="acme"
      slug="web"
      projectId="project_1"
      sessions={[A, B]}
      window={{ days: 30 }}
      layouts={[]}
      project={{ view_id: null }}
      canSave={false}
    />,
  );
}

const peeked = () => new URL(window.location.href).searchParams.get("peek");

describe("SessionPeek", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockShell = document.body;
    window.localStorage.removeItem(TRACE_PREFS_KEY);
    answers = {
      [A]: () => Response.json(detail(A)),
      [B]: () => Response.json(detail(B)),
    };
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.replaceState(null, "", "/");
  });

  it("stands shut while the URL names no session", () => {
    mockSearch = new URLSearchParams("days=30");
    peek();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("opens for an id of 200 characters, counted as the server counts them, and stands shut past them", async () => {
    const wide = "😀".repeat(200);
    answers[wide] = () => Response.json(detail(wide));
    open(wide);
    const { unmount } = peek();
    expect(await within(screen.getByRole("dialog")).findByRole("table")).toBeInTheDocument();
    unmount();
    open(`${wide}😀`);
    peek();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens beside the list and reads the session by the project's id in the list's window, its runs under the default layout", async () => {
    open(B);
    peek();
    const panel = screen.getByRole("dialog");
    expect(panel).toHaveAttribute("aria-modal", "false");
    expect(document.activeElement).toBe(panel);
    expect(within(panel).getByRole("heading", { level: 2 })).toHaveTextContent(B);
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/chat%2F42%20b?project=project_1&days=30",
      expect.anything(),
    );
    const table = await within(panel).findByRole("table");
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual([
      "",
      "Trace",
      "Started",
      "User",
      "Status",
      "Error",
      "Tokens",
      "Duration",
      "Models",
      "Tools",
    ]);
    expect(within(table).getByText("RuntimeError")).toBeInTheDocument();
    // A run that ended in error says so once: no failed-span count beside it.
    expect(within(table).queryByText(/failed span/)).toBeNull();
    // The sums on the header's line: the user, leading to their traces.
    expect(within(panel).getByRole("link", { name: `user ${HASHED}` })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}&days=30`,
    );
    expect(within(panel).getByText("1 trace")).toHaveTextContent("1 trace, 1 with failed spans");
    expect(within(panel).getByText("1,500 tokens")).toHaveAttribute("title", "1,200 in, 300 out");
    expect(within(panel).getByRole("link", { name: "Open the session's page" })).toHaveAttribute(
      "href",
      "/acme/web/sessions/chat%2F42%20b?days=30",
    );
    expect(within(panel).getByRole("button", { name: "Customize the layout" })).toBeInTheDocument();
    // A trace's line alone: nothing read beyond the session.
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(cover).not.toHaveBeenCalled();
  });

  it("steps to the next session in place, and holds still at the list's start", async () => {
    open(A);
    peek();
    const panel = screen.getByRole("dialog");
    await within(panel).findByRole("table");
    const previous = within(panel).getByRole("button", { name: "Previous session" });
    expect(previous).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(previous);
    expect(peeked()).toBe(A);

    const length = window.history.length;
    fireEvent.click(within(panel).getByRole("button", { name: "Next session" }));
    expect(peeked()).toBe(B);
    expect(window.history.length, "a step is the same entry").toBe(length);
    expect(new URLSearchParams(window.location.search).get("days")).toBe("30");
  });

  it("fills the content area at a press, the list then out of reach", async () => {
    open(A);
    peek();
    const panel = screen.getByRole("dialog");
    fireEvent.click(within(panel).getByRole("button", { name: "Full size" }));
    expect(panel).toHaveAttribute("data-mode", "full");
    expect(cover).toHaveBeenCalledTimes(1);
    expect(within(panel).getByRole("button", { name: "Exit full size" })).toBeInTheDocument();
  });

  it("shuts on Escape from inside, rewriting a link's URL without the session", async () => {
    open(A);
    peek();
    const panel = screen.getByRole("dialog");
    await within(panel).findByRole("table");
    fireEvent.keyDown(panel, { key: "Escape" });
    expect(peeked()).toBeNull();
    expect(window.location.search).toBe("?days=30");
  });

  it("lands the browser's own Back on the list as the peek last stood, though Next unmounts the panel first", async () => {
    // The list's entry under a saved layout's link, and the peek pushed from it and customized since.
    window.history.replaceState(null, "", "/acme/web/sessions?days=30&view=layout_1");
    window.history.pushState({ telmoniPeek: true }, "", `/acme/web/sessions?days=30&peek=${A}`);
    mockSearch = new URLSearchParams({ days: "30", peek: A });
    // As Next's own listener, registered first, commits the list inside the traversal.
    let unmountPanel = () => {};
    const commit = () => unmountPanel();
    window.addEventListener("popstate", commit, { once: true });
    unmountPanel = peek().unmount;
    await within(screen.getByRole("dialog")).findByRole("table");
    const landed = new Promise((resolve) => window.addEventListener("popstate", resolve, { once: true }));
    window.history.back();
    await landed;
    expect(window.location.search).toBe("?days=30");
  });

  it("keeps the landing for the list across a Back between two of the peek's own entries", async () => {
    // The list under a saved layout's link, the peek pushed from it and customized, and a new
    // window pushed while it is open.
    window.history.replaceState(null, "", "/acme/web/sessions?days=30&view=layout_1");
    window.history.pushState({ telmoniPeek: true }, "", `/acme/web/sessions?days=30&peek=${A}`);
    window.history.pushState(null, "", `/acme/web/sessions?days=7&peek=${A}`);
    mockSearch = new URLSearchParams({ days: "7", peek: A });
    const ui = () => (
      <SessionPeek
        organization="acme"
        slug="web"
        projectId="project_1"
        sessions={[A, B]}
        window={{ days: 30 }}
        layouts={[]}
        project={{ view_id: null }}
        canSave={false}
      />
    );
    // As Next's own listener, registered first: onto a peek it draws the panel again, onto the
    // list it unmounts it.
    let view: ReturnType<typeof render> | null = null;
    const commit = () => {
      if (new URLSearchParams(window.location.search).has("peek")) {
        mockSearch = new URLSearchParams(window.location.search);
        view?.rerender(ui());
      } else view?.unmount();
    };
    window.addEventListener("popstate", commit);
    view = render(ui());
    await within(screen.getByRole("dialog")).findByRole("table");
    const back = () => {
      const landed = new Promise((resolve) => window.addEventListener("popstate", resolve, { once: true }));
      window.history.back();
      return landed;
    };
    await back();
    expect(window.location.search).toBe(`?days=30&peek=${A}`);
    await back();
    window.removeEventListener("popstate", commit);
    expect(window.location.search).toBe("?days=30");
  });

  it("says when the session is not there, and when the sign-in has ended, keeping the list's query", async () => {
    answers[A] = () => new Response("{}", { status: 404 });
    open(A);
    const { unmount } = peek();
    expect(
      await screen.findByText("No session with this id in this project's window."),
    ).toBeInTheDocument();
    unmount();

    answers[A] = () => new Response("{}", { status: 401 });
    peek();
    const again = await screen.findByRole("link", { name: "Sign in again" });
    expect(again).toHaveAttribute(
      "href",
      `/auth/login?returnTo=${encodeURIComponent(`/acme/web/sessions?days=30&peek=${A}`)}`,
    );
  });

  it("offers a retry when the session could not be read", async () => {
    answers[A] = () => new Response("{}", { status: 503 });
    open(A);
    peek();
    fireEvent.click(await screen.findByRole("button", { name: "Try again" }));
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("reads the session once for a pin changed, its rows taking the panel's read", async () => {
    open(A);
    peek();
    await screen.findByRole("table");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    answers[A] = () => Response.json({ ...detail(A), pinned: ["answer"] });
    act(() => setTracePrefs({ layout: { view: null, options: { ...DEFAULT_LAYOUT, pinned: ["answer"] } } }));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    expect(String(fetchMock.mock.calls[1]![0])).toContain("pinned=answer");
    // Any read the rows made of their own would have gone by now.
    await act(() => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("reads the session again as the project's runs land, the list beside it redrawn", async () => {
    open(A);
    peek();
    await screen.findByRole("table");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    act(() => {
      window.dispatchEvent(new Event(PEEK_NUDGED));
    });
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
  });
});
