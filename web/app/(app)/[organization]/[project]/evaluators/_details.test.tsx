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
  state: null,
  questions: null,
  reasoning: true,
  media: false,
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
        agreement={null}
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

  it("reads its agreement with members: a share, a kappa and a matrix, or a mean difference", () => {
    const base = {
      name: "relevant",
      days: 30,
      agreeing: null,
      share: null,
      kappa: null,
      labels: [] as string[],
      matrix: [] as number[][],
      mean_difference: null,
    };
    const { unmount } = render(
      <EvaluatorDetails
        versions={[VERSION]}
        evaluations={null}
        agreement={{
          days: 30,
          scores: [
            {
              ...base,
              data_type: "boolean",
              pairs: 4,
              agreeing: 3,
              share: 0.75,
              kappa: 0.5,
              labels: ["yes", "no"],
              matrix: [
                [1, 1],
                [0, 2],
              ],
            },
          ],
        }}
        here="/acme/web/evaluators"
        tracesHref="/acme/web/traces"
        sessionsHref="/acme/web/sessions"
      />,
    );
    const agreed = screen.getByRole("region", { name: "Agreement with members, the last 30 days" });
    expect(agreed.textContent).toContain("4 pairs · 75% agree · Cohen’s kappa 0.50");
    expect(screen.getAllByRole("row").map((row) => row.textContent)).toEqual([
      "It, by membersyesno",
      "yes11",
      "no02",
    ]);
    unmount();
    render(
      <EvaluatorDetails
        versions={[VERSION]}
        evaluations={null}
        agreement={{
          days: 30,
          scores: [
            { ...base, name: "followed", data_type: "boolean", pairs: 0 },
            { ...base, data_type: "number", pairs: 1, mean_difference: -0.25 },
          ],
        }}
        here="/acme/web/evaluators"
        tracesHref="/acme/web/traces"
        sessionsHref="/acme/web/sessions"
      />,
    );
    const read = screen.getByRole("region", { name: /^Agreement/ }).textContent;
    expect(read).toContain("relevant1 pair · it scores 0.25 below its members on average");
    expect(read).not.toContain("followed");
  });
});
