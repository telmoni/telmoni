// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("./actions", () => ({
  saveRuleAction: vi.fn(async () => ({ ok: true })),
  previewRuleAction: vi.fn(async () => ({
    ok: true,
    preview: { days: 7, targets: 40, judged: 10, evaluators: [{ id: "e", name: "Relevant", model: "judge-1", cost: null }] },
  })),
  deleteRuleAction: vi.fn(async () => ({ ok: true })),
  saveSessionIdleAction: vi.fn(async () => ({ ok: true })),
  saveEvaluatorAction: vi.fn(),
  tryEvaluatorAction: vi.fn(),
  deleteEvaluatorAction: vi.fn(),
}));

import type { EvaluationRule, Evaluator } from "@/lib/server/entities/evaluators";
import { EvaluationTarget, ScoreType, SpanKind } from "@/lib/types/enums";

import { Rules, ruleInputOf } from "./_rules";

const EVALUATOR = {
  id: "5d4f5c68-0000-4000-8000-0000000000e1",
  version: 1,
  name: "Relevant",
  kind: "prompt",
  messages: [{ role: "user", content: "{{input}}" }],
  score: {
    name: "relevant",
    data_type: ScoreType.Boolean,
    range_min: null,
    range_max: null,
    low_means: "no",
    high_means: "yes",
    categories: [],
    multiple: false,
  },
  reasoning: true,
  connection_id: null,
  model: null,
  temperature: null,
  created_by: "u",
  created_at: "2026-11-02T00:00:00Z",
} satisfies Evaluator;

const RULE: EvaluationRule = {
  id: "5d4f5c68-0000-4000-8000-0000000000a1",
  name: "Tool calls",
  target: EvaluationTarget.Span,
  span_kind: SpanKind.ToolCall,
  span_name: "search",
  query: { agent: ["support"] },
  sample_rate: 0.25,
  pointers: {},
  enabled: false,
  backfill_from: null,
  evaluators: [EVALUATOR.id],
  created_by: "u",
  created_at: "2026-11-02T00:00:00Z",
  updated_at: "2026-11-02T00:00:00Z",
};

describe("Rules", () => {
  it("sends a span's kind and name only on a span rule, and the share as a rate", async () => {
    const base = {
      name: " Runs ",
      target: EvaluationTarget.Run,
      spanKind: SpanKind.ToolCall,
      spanName: "search",
      viewId: "",
      query: {},
      percent: "25",
      pointers: { input: "/0", output: " " },
      evaluators: [EVALUATOR.id],
      enabled: true,
      backfillFrom: "2026-10-30",
    } as const;
    const sent = ruleInputOf({ ...base, pointers: { ...base.pointers }, evaluators: [...base.evaluators] });
    expect(sent).toMatchObject({ name: "Runs", span_kind: null, span_name: null, sample_rate: 0.25 });
    expect(sent.pointers).toEqual({ input: "/0" });
    expect(sent.backfill_from).toBe("2026-10-30T00:00:00Z");
  });

  it("lists each rule by what it judges, its share and its evaluators, and previews a new one", async () => {
    render(
      <Rules projectId="p" rules={[RULE]} evaluators={[EVALUATOR]} views={[]} idleMinutes={10} canEdit contentKept />,
    );
    expect(screen.getByText(/Tool calls named search · 25% · Relevant/)).toBeTruthy();
    expect(screen.getByText("Off")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "New rule" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Every run" } });
    fireEvent.click(screen.getByRole("checkbox", { name: "Relevant" }));
    fireEvent.click(screen.getByRole("button", { name: "Preview the last 7 days" }));
    expect(await screen.findByText("40 found over the last 7 days; 10 judged at this share.")).toBeTruthy();
    expect(screen.getByText("Relevant on judge-1: no price for this model")).toBeTruthy();
  });

  it("offers a member no change", () => {
    render(
      <Rules projectId="p" rules={[RULE]} evaluators={[EVALUATOR]} views={[]} idleMinutes={10} canEdit={false} contentKept />,
    );
    expect(screen.queryByRole("button", { name: "New rule" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    expect(screen.getByLabelText(/A session is judged/)).toBeDisabled();
  });
});
