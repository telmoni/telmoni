// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { TraceDetail, TraceSpan } from "@/lib/server/entities/traces";
import { nextSpan } from "@/lib/span-fixture";
import { SpanKind } from "@/lib/types/enums";

import { COMPARE_ROWS_MAX, compareRuns, figuresOf } from "./trace-compare";

const { readTrace } = vi.hoisted(() => ({ readTrace: vi.fn() }));
vi.mock("@/components/traces/peek-read", async (actual) => ({
  ...(await actual<typeof import("@/components/traces/peek-read")>()),
  readTrace: (...args: unknown[]) => readTrace(...args),
}));

import { CompareRuns } from "@/components/traces/compare-runs";

const RUN_A = "0b6f1c1e-1d7b-4c43-9a59-1f6a3c2d4e5f";
const RUN_B = "7c2e9d4a-5b3f-4e21-8d6c-2a1b3c4d5e6f";

const timed = nextSpan();
const span = (over: Partial<TraceSpan> & Pick<TraceSpan, "span_id">) =>
  timed({ agent: "support", kind: SpanKind.Step, ...over });

/** A run of the support agent through the steps named, each under the run. */
function runThrough(runId: string, steps: string[], over: Partial<TraceDetail> = {}): TraceDetail {
  const root = span({ span_id: `${runId}-run`, kind: SpanKind.Run, name: "invoke_agent" });
  return {
    run_id: runId,
    spans: [
      root,
      ...steps.map((name, i) =>
        span({ span_id: `${runId}-${i}`, parent_span_id: root.span_id, name, input_tokens: 10 * (i + 1) }),
      ),
    ],
    truncated: false,
    spend: { spent: 0.01, unpriced: [] },
    content: null,
    ...over,
  } as TraceDetail;
}

const first = () => runThrough(RUN_A, ["plan", "search", "answer"]);
const second = () => runThrough(RUN_B, ["plan", "search", "escalate"]);

describe("compareRuns", () => {
  /** The roadmap's proof: two runs that part at their third step. */
  it("marks the first span where two runs part, at their third step", () => {
    const { parted } = compareRuns(first(), second());
    expect(parted?.at).toBe(3);
    expect(parted?.a?.span.name).toBe("answer");
    expect(parted?.b?.span.name).toBe("escalate");
  });

  it("reads two runs alike as parting nowhere, and a longer one as parting where the other ends", () => {
    expect(compareRuns(first(), first()).parted).toBeNull();
    const longer = runThrough(RUN_B, ["plan", "search", "answer", "follow_up"]);
    const { parted, partial } = compareRuns(first(), longer);
    expect(parted).toEqual({ at: 4, a: null, b: expect.objectContaining({ depth: 1 }) });
    expect(partial).toBe(false);
  });

  it("tells a step from a tool call of the same name", () => {
    const tool = runThrough(RUN_B, ["plan", "search", "answer"]);
    tool.spans[2] = { ...tool.spans[2]!, kind: SpanKind.ToolCall };
    expect(compareRuns(first(), tool).parted?.at).toBe(2);
  });

  it("draws at most its cap of rows, and finds where they part past it", () => {
    const many = Array.from({ length: COMPARE_ROWS_MAX + 9 }, (_, i) => `step-${i}`);
    const { rows, hidden, parted } = compareRuns(
      runThrough(RUN_A, many),
      runThrough(RUN_B, [...many.slice(0, -1), "other"]),
    );
    expect(rows).toHaveLength(COMPARE_ROWS_MAX);
    expect(hidden).toBe(10);
    expect(parted).toMatchObject({ at: COMPARE_ROWS_MAX + 9, a: { span: { name: `step-${COMPARE_ROWS_MAX + 8}` } } });
  });

  it("reads a run cut short by the server as parting nowhere it can see", () => {
    const cut = runThrough(RUN_B, ["plan", "search"], { truncated: true });
    expect(compareRuns(first(), cut)).toMatchObject({ parted: null, partial: true });
    // Alike up to the cap is alike only as far as both were sent.
    const both = runThrough(RUN_B, ["plan", "search", "answer"], { truncated: true });
    expect(compareRuns(first(), both)).toMatchObject({ parted: null, partial: true });
    // Where the runs part before the cut, they part there, as far as was sent.
    const early = runThrough(RUN_B, ["plan", "escalate"], { truncated: true });
    expect(compareRuns(first(), early)).toMatchObject({ parted: { at: 2 }, partial: true });
  });

  it("sums each run's figures, its cost unknown where its spend could not be read", () => {
    expect(figuresOf(first())).toMatchObject({ steps: 3, inputTokens: 0, cost: 0.01 });
    expect(figuresOf({ ...first(), spend: null }).cost).toBeNull();
  });
});

describe("CompareRuns", () => {
  afterEach(() => {
    cleanup();
    readTrace.mockReset();
  });

  const draw = (keepsContent: boolean) =>
    render(
      <CompareRuns
        a={first()}
        b={second()}
        organization="acme"
        slug="support"
        projectId="project_7bQx2mNv9BcK4dLp"
        keepsContent={keepsContent}
      />,
    );

  it("names where the runs part and marks that row", () => {
    draw(false);
    expect(screen.getByText("They part at span 4: step answer against step escalate.")).toBeInTheDocument();
    const marked = screen.getAllByRole("row").filter((row) => row.getAttribute("aria-current") === "true");
    expect(marked).toHaveLength(1);
    expect(marked[0]).toHaveTextContent("They part here");
    expect(screen.queryByRole("button", { name: "Open messages" })).toBeNull();
  });

  it("says when one run holds more spans than were sent", () => {
    render(
      <CompareRuns
        a={first()}
        b={runThrough(RUN_B, ["plan", "search"], { truncated: true })}
        organization="acme"
        slug="support"
        projectId="project_7bQx2mNv9BcK4dLp"
        keepsContent={false}
      />,
    );
    expect(screen.getByText("Alike by name and kind as far as both were sent.")).toBeInTheDocument();
    expect(screen.getByText(/The second run holds more spans than the server sends at once/)).toBeInTheDocument();
  });

  it("opens no content until asked, then reads each run through its trace's route", async () => {
    const messages = (runId: string, text: string) => [
      { run_id: runId, span_id: `${runId}-2`, fields: { "gen_ai.output.messages": JSON.stringify([{ role: "assistant", parts: [{ type: "text", content: text }] }]) } },
    ];
    readTrace.mockImplementation(async (url: string) => {
      const runId = url.includes(RUN_A) ? RUN_A : RUN_B;
      const trace = runId === RUN_A ? first() : second();
      return { kind: "ok", value: { ...trace, content: messages(runId, runId === RUN_A ? "Here it is" : "Passing you on") } };
    });
    draw(true);
    expect(readTrace).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Open messages" }));
    await waitFor(() => expect(screen.getByText("Here it is")).toBeInTheDocument());
    expect(screen.getByText("Passing you on")).toBeInTheDocument();
    expect(readTrace.mock.calls.map(([url]) => url).sort()).toEqual(
      [
        `/api/v1/traces/${RUN_A}?project=project_7bQx2mNv9BcK4dLp`,
        `/api/v1/traces/${RUN_B}?project=project_7bQx2mNv9BcK4dLp`,
      ].sort(),
    );
  });

  it("says so where the server answers a run but could not open its content", async () => {
    readTrace.mockImplementation(async (url: string) => ({
      kind: "ok",
      value: url.includes(RUN_A) ? { ...first(), content: [] } : second(),
    }));
    draw(true);
    fireEvent.click(screen.getByRole("button", { name: "Open messages" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("could not be opened"));
  });

  it("shows neither run's messages where one could not be opened", async () => {
    readTrace.mockImplementation(async (url: string) =>
      url.includes(RUN_A) ? { kind: "ok", value: first() } : { kind: "forbidden", error: null },
    );
    draw(true);
    fireEvent.click(screen.getByRole("button", { name: "Open messages" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("could not be opened"));
    expect(screen.getByRole("button", { name: "Try opening again" })).toBeInTheDocument();
  });
});
