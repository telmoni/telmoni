// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/app/(app)/[organization]/[project]/traces/actions", () => ({
  putScoreAction: vi.fn(async () => ({ ok: true })),
  deleteScoreAction: vi.fn(async () => ({ ok: true })),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import { putScoreAction } from "@/app/(app)/[organization]/[project]/traces/actions";
import { SpanScores, TraceScores, TraceScoresProvider } from "@/components/traces/trace-scores";
import type { Score } from "@/lib/server/entities/scores";

const RUN = "5d4f5c68-0000-4000-8000-00000000000a";
const SCORE: Score = {
  score_id: "7a1c0e52-0000-4000-8000-000000000002",
  target: "run",
  run_id: RUN,
  name: "useful",
  data_type: "boolean",
  value: 1,
  categories: [],
  source: "member",
  scorer: "user_me",
  given_at: "2026-11-02T10:00:00Z",
} as Score;

describe("a trace's scores", () => {
  it("opens a score's comment from its badge, and draws none for one without", () => {
    render(
      <TraceScores
        projectId="p"
        runId={RUN}
        scores={[SCORE, { ...SCORE, score_id: "other", name: "plain" }]}
        comments={{ [SCORE.score_id]: "The refund was right." }}
        spanNames={{}}
        me="user_me"
        administers={false}
        keepsContent
      />,
    );
    expect(screen.queryByRole("button", { name: "The comment on plain" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "The comment on useful" }));
    expect(screen.getByText("The refund was right.")).toBeTruthy();
  });

  it("sends a comment with a score, trimmed, and none where it is left blank", async () => {
    render(
      <TraceScores projectId="p" runId={RUN} scores={[]} comments={{}} spanNames={{}} me="user_me" administers={false} keepsContent />,
    );
    const give = async (comment: string) => {
      if (!screen.queryByRole("dialog")) fireEvent.click(screen.getByRole("button", { name: "Score" }));
      const form = within(await screen.findByRole("dialog"));
      fireEvent.change(form.getByLabelText("Name"), { target: { value: "useful" } });
      fireEvent.click(form.getByLabelText("Yes or no"));
      fireEvent.change(form.getByLabelText("Comment"), { target: { value: comment } });
      fireEvent.click(form.getByRole("button", { name: "Score" }));
    };
    await give("  Right refund.  ");
    await waitFor(() =>
      expect(putScoreAction).toHaveBeenCalledWith("p", {
        run_id: RUN,
        name: "useful",
        data_type: "boolean",
        value: 1,
        comment: "Right refund.",
      }),
    );
    vi.mocked(putScoreAction).mockClear();
    await give("   ");
    await waitFor(() =>
      expect(putScoreAction).toHaveBeenCalledWith("p", {
        run_id: RUN,
        name: "useful",
        data_type: "boolean",
        value: 1,
      }),
    );
  });

  it("asks no comment where the project keeps no content", () => {
    render(
      <TraceScores projectId="p" runId={RUN} scores={[]} comments={{}} spanNames={{}} me="user_me" administers={false} keepsContent={false} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Score" }));
    expect(screen.queryByLabelText("Comment")).toBeNull();
  });

  it("draws a span's own scores beside it, and none outside the trace's page", () => {
    const spanScore = { ...SCORE, score_id: "s1", target: "span", span_id: "00f067aa0ba902b7", name: "grounded" } as Score;
    render(
      <TraceScoresProvider scores={[SCORE, spanScore]} comments={{}}>
        <div data-testid="one">
          <SpanScores spanId="00f067aa0ba902b7" />
        </div>
        <div data-testid="other">
          <SpanScores spanId="01f067aa0ba902b8" />
        </div>
      </TraceScoresProvider>,
    );
    expect(screen.getByTestId("one").textContent).toContain("grounded");
    expect(screen.getByTestId("one").textContent).not.toContain("useful");
    expect(screen.getByTestId("other").textContent).toBe("");
    const { container } = render(<SpanScores spanId="00f067aa0ba902b7" />);
    expect(container.textContent).toBe("");
  });
});
