// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
  usePathname: () => "/acme/web",
}));

let mockShell: HTMLElement | null = null;
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: mockShell, cover: () => () => {} }),
}));

// A plain select, which jsdom can pick from.
vi.mock("@/components/ui/select", () => ({
  Select: ({
    value,
    onValueChange,
    children,
  }: {
    value: string;
    onValueChange: (v: string) => void;
    children: React.ReactNode;
  }) => (
    <select value={value} onChange={(e) => onValueChange(e.target.value)}>
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  SelectItem: ({ value, children }: { value: string; children: React.ReactNode }) => (
    <option value={value}>{children}</option>
  ),
}));

import { VersionsHost, VersionsOpener } from "./_versions-panel";

const version = (value: string, patch: Record<string, unknown> = {}) => ({
  value,
  first_at: "2026-11-05T09:00:00Z",
  traces: 10,
  failed_traces: 2,
  p50_ms: 2_100,
  p95_ms: 6_000,
  median_spend: 0.031,
  scores: [{ name: "helpful", mean: 0.8, count: 5 }],
  ...patch,
});

const answer = (patch: Record<string, unknown> = {}) => ({
  days: 14,
  by: "release",
  environment: "production",
  environments: [{ value: "production", traces: 21 }],
  prompt: null,
  prompts: [],
  versions: [
    version("1.4.3", { traces: 1, failed_traces: 0, p50_ms: null, p95_ms: null, median_spend: null, scores: [] }),
    version("1.4.2", {
      failed_traces: 4,
      p50_ms: 2_400,
      p95_ms: 9_100,
      median_spend: 0.042,
      scores: [{ name: "helpful", mean: 0.6, count: 5 }],
    }),
    version("1.4.1"),
    version("", { traces: 2, failed_traces: 0, scores: [] }),
  ],
  scores: ["helpful"],
  more: false,
  ...patch,
});

/** What each read is answered, by what it asked. */
let answered: (asked: URLSearchParams) => Response;
const fetchMock = vi.fn(async (url: string) => answered(new URL(url, "http://localhost").searchParams));

/** What the reads asked, the newest last. */
const asks = () =>
  fetchMock.mock.calls.map(([url]) => Object.fromEntries(new URL(url, "http://localhost").searchParams));

/** The table's host over a button for each agent, as the Agents page stands them. */
function hosted(...agents: string[]) {
  return render(
    <VersionsHost projectId="project_1" days={14}>
      {agents.map((agent) => (
        <VersionsOpener key={agent} agent={agent} />
      ))}
    </VersionsHost>,
  );
}

function opened(agent = "support-bot") {
  hosted(agent);
  fireEvent.click(screen.getByRole("button", { name: `Versions of ${agent}` }));
  return screen.getByRole("dialog");
}

/** A read held until it is let go, to look at the panel while it is out. */
function held() {
  let release: (response: Response) => void = () => {};
  const response = new Promise<Response>((resolve) => {
    release = resolve;
  });
  return { response, release };
}

describe("an agent's versions panel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockShell = document.body;
    answered = () => Response.json(answer());
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("stays shut, and reads nothing, until its button is pressed", () => {
    hosted("support-bot");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("is the table's one panel: another row's button opens it on that agent in place of the first", async () => {
    hosted("support-bot", "nightly-report");
    const first = screen.getByRole("button", { name: "Versions of support-bot" });
    const second = screen.getByRole("button", { name: "Versions of nightly-report" });
    fireEvent.click(first);
    await within(screen.getByRole("dialog")).findByText("1.4.2");

    fireEvent.click(second);
    const panels = screen.getAllByRole("dialog");
    expect(panels).toHaveLength(1);
    expect(within(panels[0]!).getByRole("heading", { level: 2 })).toHaveTextContent("nightly-report");
    await within(panels[0]!).findByText("1.4.2");
    expect(asks().map((ask) => ask.agent)).toEqual(["support-bot", "nightly-report"]);

    // Shut, it hands the focus back to the button that opened it.
    fireEvent.click(within(panels[0]!).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(second).toHaveFocus());
  });

  it("is no button without the table's panel to open", () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => render(<VersionsOpener agent="support-bot" />)).toThrow(/VersionsHost/);
    quiet.mockRestore();
  });

  it("opens beside the page on the agent's releases, a row each with its change from the row below", async () => {
    const panel = opened();
    expect(panel).toHaveAttribute("aria-modal", "false");
    expect(within(panel).getByRole("heading", { level: 2 })).toHaveTextContent("support-bot");
    expect(within(panel).getByText("Reading its versions…")).toBeInTheDocument();
    expect(within(panel).getByRole("tab", { name: "Releases" })).toHaveAttribute("aria-selected", "true");

    const newer = (await within(panel).findByText("1.4.2")).closest("tr")!;
    // Over the days its page read, which the read names.
    expect(asks()).toEqual([{ agent: "support-bot", by: "release", days: "14", project: "project_1" }]);
    expect(within(panel).getAllByRole("columnheader").map((th) => th.textContent)).toEqual([
      "Release",
      "Traces",
      "Failed",
      "Median",
      "p95",
      "Median cost",
      "helpful",
    ]);
    expect(within(newer).getByText("40%")).toBeInTheDocument();
    expect(within(newer).getByText("+20 pts")).toHaveClass("text-destructive");
    expect(within(newer).getByText("+300 ms")).toHaveClass("text-destructive");
    expect(within(newer).getByText("+3.1 s")).toHaveClass("text-destructive");
    expect(within(newer).getByText("+$0.011")).toHaveClass("text-destructive");
    // Whether a score is better higher is not known: its change is not judged.
    expect(within(newer).getByText("−0.20")).toHaveClass("text-muted-foreground");
    expect(within(newer).getByText(/First seen/)).toBeInTheDocument();

    // No ended trace: no duration or cost, and no change drawn from none, its
    // share of failures among them: one trace still running has not fallen 40 points.
    const newest = within(panel).getByText("1.4.3").closest("tr")!;
    expect(within(newest).getAllByText("—")).toHaveLength(4);
    expect(within(newest).getByText("0%")).toBeInTheDocument();
    expect(within(newest).queryByText(/pts|No change|ms|\$/)).toBeNull();

    // The oldest has no row below to change from, and none is measured against no version.
    const oldest = within(panel).getByText("1.4.1").closest("tr")!;
    expect(within(oldest).queryByText(/pts|No change/)).toBeNull();
    const none = within(panel).getByText("(none)").closest("tr")!;
    expect(within(none).getByText("Traces that carried no release")).toBeInTheDocument();
    expect(within(none).queryByText(/pts|No change/)).toBeNull();
    expect(within(panel).getAllByRole("row").at(-1)).toBe(none);

    expect(within(panel).getByText(/over the last 14 days.*not when it was deployed/)).toBeInTheDocument();
    expect(within(panel).queryByRole("combobox")).toBeNull();
  });

  it("says an unchanged number is unchanged", async () => {
    answered = () => Response.json(answer({ versions: [version("b"), version("a")], scores: [] }));
    const panel = opened();
    const newer = (await within(panel).findByText("b")).closest("tr")!;
    expect(within(newer).getAllByText("No change")).toHaveLength(4);
  });

  it("says a number is unchanged where its change is written as zero, and judges none", async () => {
    answered = () =>
      Response.json(
        answer({
          versions: [
            // 12.34% against 12.30%, and a mean off by the arithmetic's last digit.
            version("b", { traces: 10_000, failed_traces: 1_234, scores: [{ name: "helpful", mean: 0.1 + 0.2, count: 5 }] }),
            version("a", { traces: 10_000, failed_traces: 1_230, scores: [{ name: "helpful", mean: 0.3, count: 5 }] }),
          ],
        }),
      );
    const panel = opened();
    const newer = (await within(panel).findByText("b")).closest("tr")!;
    const unchanged = within(newer).getAllByText("No change");
    expect(unchanged).toHaveLength(5);
    for (const each of unchanged) expect(each).toHaveClass("text-muted-foreground");
    expect(within(newer).queryByText(/pts|[+−]/)).toBeNull();
  });

  it("asks for an agent by a parameter, whatever its name", async () => {
    const panel = opened("..");
    await within(panel).findByText("1.4.2");
    const [url] = fetchMock.mock.calls[0]!;
    expect(new URL(url, "http://localhost").pathname).toBe("/api/v1/agent-versions");
    expect(asks()[0]).toEqual({ agent: "..", by: "release", days: "14", project: "project_1" });
  });

  it("reads its models, and says a trace counts under each it called", async () => {
    answered = (asked) =>
      Response.json(
        asked.get("by") === "model"
          ? answer({ by: "model", versions: [version("gpt-4o"), version("gpt-4o-mini")] })
          : answer(),
      );
    const panel = opened();
    await within(panel).findByText("1.4.2");
    fireEvent.mouseDown(within(panel).getByRole("tab", { name: "Models" }), { button: 0 });
    expect(await within(panel).findByText("gpt-4o-mini")).toBeInTheDocument();
    expect(asks().at(-1)).toEqual({ agent: "support-bot", by: "model", days: "14", project: "project_1" });
    expect(within(panel).getByRole("columnheader", { name: "Model" })).toBeInTheDocument();
    expect(within(panel).getByText(/A trace counts under every model it called\./)).toBeInTheDocument();
  });

  it("offers the environments where there are two, the one with no name among them, and reads the one picked", async () => {
    const environments = [
      { value: "production", traces: 21 },
      { value: "", traces: 2 },
    ];
    answered = (asked) =>
      Response.json(answer({ environments, environment: asked.get("environment") ?? "production" }));
    const panel = opened();
    await within(panel).findByText("1.4.2");
    const picker = within(panel).getByRole("combobox");
    expect(within(picker).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "production",
      "(no environment)",
    ]);
    expect(picker).toHaveValue("0");

    fireEvent.change(picker, { target: { value: "1" } });
    await waitFor(() =>
      expect(asks().at(-1)).toEqual({
        agent: "support-bot",
        by: "release",
        environment: "",
        days: "14",
        project: "project_1",
      }),
    );
    expect(await within(panel).findByText(/whose spans in \(no environment\)/)).toBeInTheDocument();
    expect(within(panel).getByRole("combobox")).toHaveValue("1");
  });

  it("shows the environment picked before its read lands, and keeps the pickers where that read fails", async () => {
    const environments = [
      { value: "production", traces: 21 },
      { value: "staging", traces: 4 },
    ];
    const read = held();
    answered = () => Response.json(answer({ environments, days: 3 }));
    const panel = opened();
    await within(panel).findByText("1.4.2");
    expect(within(panel).getByText(/side by side over the last 3 days\./)).toBeInTheDocument();

    fetchMock.mockImplementationOnce(() => read.response);
    fireEvent.change(within(panel).getByRole("combobox"), { target: { value: "1" } });
    // Out and not yet answered: the pick stands, over the note that it is being read.
    expect(within(panel).getByRole("combobox")).toHaveValue("1");
    expect(within(panel).getByText("Reading its versions…")).toBeInTheDocument();
    expect(asks().at(-1)?.environment).toBe("staging");

    read.release(new Response("{}", { status: 503 }));
    expect(await within(panel).findByText("The versions could not be read.")).toBeInTheDocument();
    expect(within(panel).getByRole("combobox")).toHaveValue("1");
    expect(within(panel).getByText(/side by side over the last 3 days\./)).toBeInTheDocument();
  });

  it("reads a prompt's versions: the one most named, then the one picked", async () => {
    const prompts = [
      { value: "triage", traces: 9 },
      { value: "triage:blue", traces: 3 },
    ];
    answered = (asked) =>
      Response.json(
        asked.get("by") === "prompt"
          ? answer({ by: "prompt", prompt: asked.get("prompt") ?? "triage", prompts, versions: [version("7"), version("6")] })
          : answer(),
      );
    const panel = opened();
    await within(panel).findByText("1.4.2");
    fireEvent.mouseDown(within(panel).getByRole("tab", { name: "Prompts" }), { button: 0 });
    expect(await within(panel).findByRole("columnheader", { name: "Version" })).toBeInTheDocument();
    expect(asks().at(-1)).toEqual({ agent: "support-bot", by: "prompt", days: "14", project: "project_1" });
    const picker = within(panel).getByRole("combobox");
    expect(picker).toHaveValue("triage");

    // The pick shows while its read is out, the picker never leaving.
    const read = held();
    fetchMock.mockImplementationOnce(() => read.response);
    fireEvent.change(picker, { target: { value: "triage:blue" } });
    expect(within(panel).getByRole("combobox")).toHaveValue("triage:blue");
    expect(within(panel).getByText("Reading its versions…")).toBeInTheDocument();
    expect(asks().at(-1)?.prompt).toBe("triage:blue");

    read.release(answered(new URLSearchParams({ by: "prompt", prompt: "triage:blue" })));
    await within(panel).findByText("7");
    expect(within(panel).getByRole("combobox")).toHaveValue("triage:blue");
  });

  it("says so where its traces named no prompt, or the window held more than it shows", async () => {
    answered = (asked) =>
      Response.json(
        asked.get("by") === "prompt" ? answer({ by: "prompt", versions: [], scores: [] }) : answer({ more: true, days: 1 }),
      );
    const panel = opened();
    expect(await within(panel).findByText(/More were seen than are shown/)).toBeInTheDocument();
    expect(within(panel).getByText(/side by side over the last day\./)).toBeInTheDocument();
    fireEvent.mouseDown(within(panel).getByRole("tab", { name: "Prompts" }), { button: 0 });
    expect(await within(panel).findByText("None of its traces in production named a prompt.")).toBeInTheDocument();
    expect(within(panel).queryByRole("table")).toBeNull();
  });

  it("says a read failed, and reads again when asked", async () => {
    answered = () => new Response("{}", { status: 503 });
    const panel = opened();
    expect(await within(panel).findByText("The versions could not be read.")).toBeInTheDocument();
    expect(within(panel).queryByRole("table")).toBeNull();

    answered = () => Response.json(answer());
    fireEvent.click(within(panel).getByRole("button", { name: "Try again" }));
    expect(await within(panel).findByText("1.4.2")).toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("says whose it is to read, and when the session has ended", async () => {
    answered = () => new Response("{}", { status: 403 });
    const panel = opened();
    expect(await within(panel).findByText("This project's traces are not yours to read.")).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();

    // Opened again it is drawn afresh, and reads again.
    answered = () => new Response("{}", { status: 401 });
    fireEvent.click(screen.getByRole("button", { name: "Versions of support-bot" }));
    const again = screen.getByRole("dialog");
    expect(await within(again).findByRole("link", { name: "Sign in again" })).toBeInTheDocument();
  });

  it("refuses an answer of another shape rather than draw it", async () => {
    answered = () => Response.json({ ...answer(), versions: [{ value: "1.0.0" }] });
    const panel = opened();
    expect(await within(panel).findByText("The versions could not be read.")).toBeInTheDocument();
  });

  it("names itself, its tabs, its pickers and its table's columns for a screen reader", async () => {
    answered = () =>
      Response.json(
        answer({
          environments: [
            { value: "production", traces: 21 },
            { value: "staging", traces: 4 },
          ],
        }),
      );
    const panel = opened();
    await within(panel).findByText("1.4.2");
    expect(panel).toHaveAccessibleName("support-bot");
    expect(within(panel).getByRole("tablist", { name: "What its traces are split by" })).toBeInTheDocument();
    expect(within(panel).getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["Releases", "Models", "Prompts"]);
    const table = within(panel).getByRole("table");
    expect(within(table).getAllByRole("columnheader")).toHaveLength(7);
    // A header and a row for each version, the traces that carried none among them.
    expect(within(table).getAllByRole("row")).toHaveLength(5);
    expect(within(panel).getByRole("button", { name: "Close" })).toBeInTheDocument();
  });
});
