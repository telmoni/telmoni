// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { Evaluation, Evaluator } from "@/lib/server/entities/evaluators";
import { EvaluationStatus, EvaluationTarget, ScoreType } from "@/lib/types/enums";

import { EvaluatorDetails } from "./_details";

const VERSION: Evaluator = {
  id: "5d4f5c68-0000-4000-8000-0000000000e1",
  version: 2,
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
  model: "judge-1",
  temperature: null,
  weekly_limit: 5,
  created_by: "u",
  created_at: "2026-11-02T00:00:00Z",
};

const EVALUATION: Evaluation = {
  id: "e1",
  evaluator_version: 2,
  rule_id: null,
  target: EvaluationTarget.Run,
  run_id: "5d4f5c68-0000-4000-8000-0000000000a1",
  span_id: null,
  session_id: null,
  status: EvaluationStatus.Failed,
  attempts: 1,
  reason: "the model answered outside the score's schema: no score",
  provider: "openai",
  model: "judge-1",
  read_at: "2026-11-02T00:00:00Z",
  cut: true,
  score_id: null,
  trace_run_id: "5d4f5c68-0000-4000-8000-0000000000b1",
  input_tokens: 120,
  output_tokens: 20,
  cost: null,
  created_at: "2026-11-02T00:00:00Z",
  settled_at: "2026-11-02T00:00:00Z",
};

describe("EvaluatorDetails", () => {
  it("counts its evaluations against its weekly limit, links each to what it judged and its own trace, and says why one failed", () => {
    render(
      <EvaluatorDetails
        versions={[VERSION]}
        evaluations={{ evaluations: [EVALUATION], pending: 0, delayed: 0, done: 3, failed: 1, spent_this_week: 0.5 }}
        here="/acme/web/evaluators"
        tracesHref="/acme/web/traces"
        sessionsHref="/acme/web/sessions"
      />,
    );
    const region = screen.getByRole("region", { name: "Evaluator details" });
    expect(region.textContent).toContain("3 done · 0 pending · 0 delayed · 1 failed");
    expect(region.textContent).toContain("a week");
    expect(screen.getByRole("link", { name: "Its trace" }).getAttribute("href")).toBe(`/acme/web/traces/${EVALUATION.run_id}`);
    expect(screen.getByRole("link", { name: "The judgement" }).getAttribute("href")).toBe(
      `/acme/web/traces/${EVALUATION.trace_run_id}`,
    );
    expect(region.textContent).toContain("judged as cut to fit an export");
    expect(region.textContent).toContain("outside the score's schema");
  });
});
