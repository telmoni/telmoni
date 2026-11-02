// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <header>
      <h1>{title}</h1>
      {action}
    </header>
  ),
}));
vi.mock("./actions", () => ({
  saveEvaluatorAction: vi.fn(async () => ({ ok: true })),
  tryEvaluatorAction: vi.fn(async () => ({ ok: true })),
  deleteEvaluatorAction: vi.fn(async () => ({ ok: true })),
}));

import type { Evaluator, EvaluatorTemplate } from "@/lib/server/entities/evaluators";
import { ScoreType } from "@/lib/types/enums";

import { Evaluators, TryResult, draftOf, inputOf } from "./_evaluators";

const SCORE = {
  name: "relevant",
  data_type: ScoreType.Boolean,
  range_min: null,
  range_max: null,
  low_means: "off topic",
  high_means: "on topic",
  categories: [],
  multiple: false,
};

const TEMPLATE: EvaluatorTemplate = {
  key: "answer_relevance",
  group: "quality",
  name: "Answer relevance",
  description: "Whether the answer addresses the request it was given.",
  messages: [
    { role: "system", content: "You judge relevance." },
    { role: "user", content: "{{input}} / {{output}}" },
  ],
  score: SCORE,
  reasoning: true,
};

const EVALUATOR: Evaluator = {
  id: "5d4f5c68-0000-4000-8000-0000000000e1",
  version: 2,
  name: "Relevant",
  kind: "prompt",
  messages: TEMPLATE.messages,
  score: SCORE,
  reasoning: true,
  connection_id: null,
  model: null,
  temperature: 0,
  created_by: "u",
  created_at: "2026-11-02T00:00:00Z",
  versions: 2,
};

const CONNECTION = {
  id: "5d4f5c68-0000-4000-8000-0000000000c1",
  name: "Gateway",
  provider: "openai_compatible" as const,
  base_url: "https://gw.example/v1",
  region: null,
  models: ["judge-1"],
  is_default: true,
  secret_hint: "WXYZ",
  created_at: "2026-11-02T00:00:00Z",
  updated_at: "2026-11-02T00:00:00Z",
};

function page(over: Partial<Parameters<typeof Evaluators>[0]> = {}) {
  return render(
    <Evaluators
      projectId="p"
      evaluators={[EVALUATOR]}
      templates={[TEMPLATE]}
      connections={[CONNECTION]}
      contentKept
      canEdit
      here="/acme/web/evaluators"
      privacyHref="/acme/web/privacy"
      settingsHref="/acme/web/settings"
      {...over}
    />,
  );
}

describe("Evaluators", () => {
  it("copies a template into a draft, and sends a score's fields its type uses alone", () => {
    const draft = draftOf(TEMPLATE);
    expect(draft.id).toBeUndefined();
    draft.messages[0]!.content = "changed";
    expect(TEMPLATE.messages[0]!.content).toBe("You judge relevance.");
    const sent = inputOf({ ...draft, model: "  ", temperature: "0.2", score: { ...draft.score, range_min: 3 } });
    expect(sent.model).toBeNull();
    expect(sent.temperature).toBe(0.2);
    expect(sent.score.range_min).toBeNull();
    expect(inputOf({ ...draft, messages: [{ role: "system", content: " " }, ...draft.messages.slice(1)] }).messages).toHaveLength(1);
  });

  it("lists each evaluator's version and versions, and offers a member no change", () => {
    page({ canEdit: false });
    expect(screen.getByText("v2")).toBeTruthy();
    expect(screen.getByRole("link", { name: "2 versions" }).getAttribute("href")).toBe(
      `/acme/web/evaluators?versions=${EVALUATOR.id}`,
    );
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("button", { name: /New evaluator/ })).toBeNull();
  });

  it("says why none can run where the project keeps no content, and offers none", () => {
    page({ contentKept: false });
    expect(screen.getByText("This project keeps no content.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /New evaluator/ })).toBeNull();
  });

  it("starts a new one from a template, its prompt and score filled in", () => {
    page();
    fireEvent.click(screen.getByRole("button", { name: /New evaluator/ }));
    fireEvent.click(screen.getByRole("button", { name: /Answer relevance/ }));
    const form = screen.getByRole("form", { name: "New evaluator" });
    expect((screen.getByLabelText("Score name") as HTMLInputElement).value).toBe("relevant");
    expect((screen.getByLabelText("Yes means") as HTMLInputElement).value).toBe("on topic");
    expect(form.textContent).toContain("Try it on a trace");
    expect(screen.getByRole("button", { name: "Try" })).toBeDisabled();
  });

  it("shows a try's score, its reasoning and the prompt as sent", () => {
    render(
      <TryResult
        tried={{
          system: "You judge relevance.",
          messages: [{ role: "user", content: "2+2? / 4" }],
          model: "judge-1",
          judgement: { name: "relevant", data_type: ScoreType.Boolean, value: 1, categories: [] },
          reasoning: "It answers.",
          refused: null,
          input_tokens: 120,
          output_tokens: 20,
        }}
      />,
    );
    expect(screen.getByRole("region", { name: "The try's result" }).textContent).toContain("relevant: yes");
    expect(screen.getByText("It answers.")).toBeTruthy();
    expect(screen.getByText("2+2? / 4")).toBeTruthy();
  });
});
