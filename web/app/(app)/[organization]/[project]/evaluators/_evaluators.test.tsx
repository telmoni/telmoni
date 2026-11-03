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

import type { Evaluator, EvaluatorTemplate, TypedTemplate } from "@/lib/server/entities/evaluators";
import { ScoreType } from "@/lib/types/enums";

import { Evaluators, TryResult, complete, draftOf, fitting, inputOf, probabilityWords } from "./_evaluators";

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
  state: null,
  questions: null,
  reasoning: true,
  media: false,
  connection_id: null,
  model: null,
  temperature: 0,
  weekly_limit: null,
  created_by: "u",
  created_at: "2026-11-02T00:00:00Z",
  versions: 2,
};

const TYPED: TypedTemplate = {
  key: "typed_groundedness",
  group: "quality",
  name: "Groundedness (typed)",
  description: "Whether the answer rests on the context.",
  state: "{{input}} / {{output}}",
  questions: [
    { name: "grounded", instructions: "Is it supported?", type: "noul", yes_means: "", no_means: "" },
    { name: "support", instructions: "How much?", type: "choice", options: [{ name: "full", means: "" }, { name: "none", means: "" }] },
  ],
};

const SYSTEM_ONE = {
  id: "5d4f5c68-0000-4000-8000-0000000000c2",
  name: "Clef",
  provider: "system_one_compatible" as const,
  base_url: "https://clef.example",
  region: null,
  models: ["Cloudflare/clef"],
  is_default: false,
  secret_hint: "",
  refused_at: null,
  refused_reason: null,
  created_at: "2026-11-02T00:00:00Z",
  updated_at: "2026-11-02T00:00:00Z",
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
  refused_at: null,
  refused_reason: null,
  created_at: "2026-11-02T00:00:00Z",
  updated_at: "2026-11-02T00:00:00Z",
};

function page(over: Partial<Parameters<typeof Evaluators>[0]> = {}) {
  return render(
    <Evaluators
      projectId="p"
      evaluators={[EVALUATOR]}
      templates={[TEMPLATE]}
      typedTemplates={[TYPED]}
      connections={[CONNECTION, SYSTEM_ONE]}
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
    if (sent.kind !== "prompt") throw new Error("a prompt's draft");
    expect(sent.model).toBeNull();
    expect(sent.temperature).toBe(0.2);
    expect(sent.score.range_min).toBeNull();
    const trimmed = inputOf({ ...draft, messages: [{ role: "system", content: " " }, ...draft.messages.slice(1)] });
    expect(trimmed.kind === "prompt" && trimmed.messages).toHaveLength(1);
  });

  it("copies a typed template into a draft, and sends its state and questions alone", () => {
    const draft = draftOf(TYPED);
    expect(draft.kind).toBe("typed");
    draft.questions[1]!.name = "changed";
    expect(TYPED.questions[1]!.name).toBe("support");
    const sent = inputOf({ ...draft, temperature: "0.5", questions: [{ ...draft.questions[0]!, name: " grounded " }] });
    expect(sent).toMatchObject({ kind: "typed", state: "{{input}} / {{output}}", questions: [{ name: "grounded", type: "noul" }] });
    expect("temperature" in sent || "messages" in sent || "score" in sent).toBe(false);
  });

  it("starts a typed one from its template, judging on a System One connection alone", () => {
    page();
    fireEvent.click(screen.getByRole("button", { name: /New evaluator/ }));
    expect(screen.getByText("Typed, on a System One model")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Groundedness \(typed\)/ }));
    const form = screen.getByRole("form", { name: "New evaluator" });
    expect((screen.getByLabelText("State") as HTMLTextAreaElement).value).toBe("{{input}} / {{output}}");
    expect(screen.getByRole("group", { name: "Question support" })).toBeTruthy();
    expect(form.textContent).toContain("cannot abstain and gives no reasons");
    expect(screen.queryByLabelText("Temperature")).toBeNull();
    expect(fitting([CONNECTION, SYSTEM_ONE], true).map((c) => c.name)).toEqual(["Clef"]);
    expect(fitting([CONNECTION, SYSTEM_ONE], false).map((c) => c.name)).toEqual(["Gateway"]);
  });

  it("holds a question whole before it is sent, and reads a level's probability by its name", () => {
    const noul = { name: "a", instructions: "b", type: "noul" as const, yes_means: "yes", no_means: "" };
    expect(complete(noul)).toBe(false);
    expect(complete({ ...noul, no_means: "no" })).toBe(true);
    expect(complete({ name: "a", instructions: "b", type: "score", levels: ["low", " "] })).toBe(false);
    expect(probabilityWords({ confidence: 0.5, probabilities: { "0": 0.25, "1": 0.75 }, model: "m" }, ["low", "high"])).toBe(
      "confidence 50% · low 25%, high 75%",
    );
  });

  it("shows each typed answer with its probabilities", () => {
    render(
      <TryResult
        tried={{
          system: "",
          messages: [{ role: "user", content: "the state" }],
          model: "jev-1.14.0",
          judgement: null,
          answers: [
            { name: "grounded", data_type: ScoreType.Boolean, value: 1, categories: [], answer: { noul: 0.82, model: "jev-1.14.0" } },
            {
              name: "support",
              data_type: ScoreType.Category,
              value: null,
              categories: ["full"],
              answer: { confidence: 0.9, probabilities: { full: 0.9, none: 0.1 }, model: "jev-1.14.0" },
            },
          ],
          reasoning: null,
          refused: null,
          input_tokens: 300,
          output_tokens: 0,
        }}
      />,
    );
    const result = screen.getByRole("region", { name: "The try's result" }).textContent;
    expect(result).toContain("grounded: yes (yes 82%)");
    expect(result).toContain("support: full (confidence 90% · full 90%, none 10%)");
    expect(result).toContain("The state as sent");
  });

  it("lists each evaluator's version and versions, and offers a member no change", () => {
    page({ canEdit: false });
    expect(screen.getByText("v2")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Evaluations · 2 versions" }).getAttribute("href")).toBe(
      `/acme/web/evaluators?evaluator=${EVALUATOR.id}`,
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
