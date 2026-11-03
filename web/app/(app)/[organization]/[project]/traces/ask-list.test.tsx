// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { push, toastMessage, toastError, askTraceListAction } = vi.hoisted(() => ({
  push: vi.fn(),
  toastMessage: vi.fn(),
  toastError: vi.fn(),
  askTraceListAction: vi.fn(),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, refresh: vi.fn() }),
}));

vi.mock("sonner", () => ({ toast: { message: toastMessage, error: toastError, success: vi.fn() } }));

vi.mock("@/app/(app)/agent-actions", () => ({
  askTraceListAction: (...args: unknown[]) => askTraceListAction(...args),
}));

import { AskList } from "@/components/traces/ask-list";
import { TRACE_QUESTION_MAX } from "@/lib/types/agent";

const PROJECT = "project_7bQx2mNv9BcK4dLp";

function ask(sentence: string, props: Partial<Parameters<typeof AskList>[0]> = {}) {
  render(
    <AskList organization="acme" slug="web" projectId={PROJECT} days={30} q={undefined} ui={{}} {...props} />,
  );
  const input = screen.getByRole("textbox", { name: "Ask the list in words" });
  fireEvent.change(input, { target: { value: sentence } });
  const form = input.closest("form");
  if (!form) throw new Error("the input sits in a form");
  return act(async () => {
    fireEvent.submit(form);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("AskList", () => {
  it("asks over the window the list shows, and opens the list on the answer", async () => {
    askTraceListAction.mockResolvedValue({
      ok: true,
      answer: {
        query: { agent: ["billing"], status: ["error"], sort: "duration", dir: "desc", days: 7 },
        unmet: null,
        dropped: [],
      },
    });
    await ask("  billing errors, slowest first, this week  ", {
      q: "refund",
      ui: { display: "chart", view: "a-view" },
    });
    expect(askTraceListAction).toHaveBeenCalledWith(PROJECT, "billing errors, slowest first, this week", 30);
    // The filters are the answer's alone; the search box's words and the
    // charts stay, and the view it was opened from no longer shows.
    expect(push).toHaveBeenCalledWith(
      "/acme/web/traces?agent=billing&status=error&sort=duration&dir=desc&days=7&q=refund&display=chart",
      { scroll: false },
    );
    expect(toastMessage).not.toHaveBeenCalled();
  });

  it("says what the filters left out, and opens nothing where the answer is none", async () => {
    askTraceListAction.mockResolvedValue({
      ok: true,
      answer: { query: null, unmet: "That could not be read as the list's filters.", dropped: [] },
    });
    await ask("make it pretty");
    expect(push).not.toHaveBeenCalled();
    expect(toastMessage).toHaveBeenCalledWith("That could not be read as the list's filters.");
  });

  it("words a refusal, and sends nothing blank", async () => {
    askTraceListAction.mockResolvedValue({ ok: false, error: "plan limit reached" });
    await ask("errors");
    expect(toastError).toHaveBeenCalledWith("plan limit reached");
    expect(push).not.toHaveBeenCalled();

    askTraceListAction.mockClear();
    cleanup();
    await ask("   ");
    expect(askTraceListAction).not.toHaveBeenCalled();
  });

  it("holds the sentence to the server's length", () => {
    render(<AskList organization="acme" slug="web" projectId={PROJECT} days={7} q={undefined} ui={{}} />);
    expect(screen.getByRole("textbox", { name: "Ask the list in words" })).toHaveAttribute(
      "maxLength",
      String(TRACE_QUESTION_MAX),
    );
  });
});
