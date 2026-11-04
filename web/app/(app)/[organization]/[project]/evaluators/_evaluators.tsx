"use client";

import { Plus } from "lucide-react";
import Link from "next/link";
import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EnumSelect } from "@/components/enum-select";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageAction } from "@/components/page-action";
import { PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { WholeListTable } from "@/components/whole-list-table";
import type {
  Evaluator,
  EvaluatorTemplate,
  PromptMessage,
  Question,
  QuestionType,
  ScoreSpec,
  Tried,
  TypedAnswer,
  TypedTemplate,
} from "@/lib/server/entities/evaluators";
import type { ModelConnection } from "@/lib/server/entities/model-connections";
import {
  EVALUATOR_MEANS_MAX,
  EVALUATOR_MESSAGES_MAX,
  EVALUATOR_NAME_MAX,
  MODEL_CONNECTION_MODEL_MAX,
  TYPED_INSTRUCTIONS_MAX,
  TYPED_LEVELS_MAX,
  TYPED_OPTIONS_MAX,
  TYPED_QUESTIONS_MAX,
  VARIABLES,
} from "@/lib/evaluators";
import { SCORE_CATEGORIES_MAX, SCORE_CATEGORY_MAX, formatScoreValue } from "@/lib/scores";
import { SCORE_NAME_MAX } from "@/lib/traces";
import { ModelProvider, type PromptRole, ScoreType, SortDirection } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";
import { decimalTyped } from "@/lib/typed-number";

import { deleteEvaluatorAction, saveEvaluatorAction, tryEvaluatorAction } from "./actions";

const GROUP_LABEL: Record<string, string> = {
  quality: "Quality",
  safety: "Safety",
  security: "Security",
  conversation: "Conversation",
  agents: "Agents",
};

const TYPE_LABEL: Record<ScoreType, string> = {
  [ScoreType.Number]: "A number on a range",
  [ScoreType.Boolean]: "Yes or no",
  [ScoreType.Category]: "Categories",
};

const ROLE_LABEL: Record<PromptRole, string> = { system: "System", user: "User", assistant: "Assistant" };

const QUESTION_LABEL: Record<QuestionType, string> = {
  noul: "Yes or no",
  score: "Ordered levels",
  choice: "One of options",
};

/** The providers that speak System One, so answer typed evaluators alone. */
export const SYSTEM_ONE: ReadonlySet<ModelProvider> = new Set([ModelProvider.Typesafe, ModelProvider.SystemOneCompatible]);

/** The connections an evaluator of a kind judges on: a System One one's for a typed evaluator, any other's for a prompt. */
export function fitting(connections: readonly ModelConnection[], typed: boolean): ModelConnection[] {
  return connections.filter((c) => SYSTEM_ONE.has(c.provider) === typed);
}

/** A draft as the form holds it: a prompt's messages and score, or a typed one's state and questions. */
export type Draft = {
  id?: string;
  name: string;
  kind: "prompt" | "typed";
  state: string;
  questions: Question[];
  messages: PromptMessage[];
  score: ScoreSpec;
  reasoning: boolean;
  media: boolean;
  connection_id: string | null;
  model: string;
  temperature: string;
  weeklyLimit: string;
};

/** A question of `type`, blank. */
function blankQuestion(type: QuestionType): Question {
  if (type === "noul") return { name: "", instructions: "", type, yes_means: "", no_means: "" };
  if (type === "score") return { name: "", instructions: "", type, levels: ["", ""] };
  return { name: "", instructions: "", type, options: [{ name: "", means: "" }, { name: "", means: "" }] };
}

const BLANK: Draft = {
  name: "",
  kind: "prompt",
  state: "",
  questions: [],
  messages: [
    { role: "system", content: "" },
    { role: "user", content: "The request:\n{{input}}\n\nThe answer:\n{{output}}" },
  ],
  score: {
    name: "",
    data_type: ScoreType.Boolean,
    range_min: null,
    range_max: null,
    low_means: "",
    high_means: "",
    categories: [],
    multiple: false,
  },
  reasoning: true,
  media: false,
  connection_id: null,
  model: "",
  temperature: "",
  weeklyLimit: "",
};

const BLANK_TYPED: Draft = {
  ...BLANK,
  kind: "typed",
  state: "The request:\n{{input}}\n\nThe answer:\n{{output}}",
  questions: [blankQuestion("noul")],
  reasoning: false,
};

/** A copy of `question`, its lists copied too. */
function copyOf(question: Question): Question {
  if (question.type === "score") return { ...question, levels: [...question.levels] };
  if (question.type === "choice") return { ...question, options: question.options.map((o) => ({ ...o })) };
  return { ...question };
}

/** A template or a saved version, as a draft to change. */
export function draftOf(from: Evaluator | EvaluatorTemplate | TypedTemplate): Draft {
  const saved = "id" in from;
  const typed = "state" in from && from.state !== null && from.questions !== null;
  const messages = "messages" in from && from.messages ? from.messages : null;
  const score = "score" in from && from.score ? from.score : null;
  return {
    id: saved ? from.id : undefined,
    name: from.name,
    kind: typed ? "typed" : "prompt",
    state: typed ? from.state! : "",
    questions: typed ? from.questions!.map(copyOf) : [],
    messages: (messages ?? BLANK.messages).map((m) => ({ ...m })),
    score: score ? { ...score, categories: score.categories.map((c) => ({ ...c })) } : { ...BLANK.score, categories: [] },
    reasoning: "reasoning" in from ? from.reasoning : false,
    media: saved ? from.media : false,
    connection_id: saved ? from.connection_id : null,
    model: saved ? (from.model ?? "") : "",
    temperature: saved && from.temperature !== null ? String(from.temperature) : "",
    weeklyLimit: saved && from.weekly_limit !== null ? String(from.weekly_limit) : "",
  };
}

/** Whether a question is whole: named, instructed, a yes or no described
 *  both ways or neither, and every level and option named. */
export function complete(question: Question): boolean {
  if (!question.name.trim() || !question.instructions.trim()) return false;
  if (question.type === "noul") return (question.yes_means.trim() === "") === (question.no_means.trim() === "");
  if (question.type === "score") return question.levels.every((l) => l.trim() !== "");
  return question.options.every((o) => o.name.trim() !== "");
}

/** A question as the lanes take it: its texts trimmed. */
function questionInput(question: Question): Question {
  const asked = { name: question.name.trim(), instructions: question.instructions.trim() };
  if (question.type === "noul") {
    return { ...asked, type: "noul", yes_means: question.yes_means.trim(), no_means: question.no_means.trim() };
  }
  if (question.type === "score") return { ...asked, type: "score", levels: question.levels.map((l) => l.trim()) };
  return { ...asked, type: "choice", options: question.options.map((o) => ({ name: o.name.trim(), means: o.means.trim() })) };
}

/** The draft as the lanes take it: what its kind does not use, and a score's
 *  fields its type does not use, left out. */
export function inputOf(draft: Draft) {
  const { data_type } = draft.score;
  const temperature = draft.temperature.trim() === "" ? null : Number(draft.temperature);
  const weekly = draft.weeklyLimit.trim() === "" ? null : Number(draft.weeklyLimit);
  const common = {
    id: draft.id,
    name: draft.name.trim(),
    connection_id: draft.connection_id,
    model: draft.model.trim() || null,
    weekly_limit: weekly !== null && Number.isFinite(weekly) && weekly > 0 ? weekly : null,
  };
  if (draft.kind === "typed") {
    return { ...common, kind: "typed" as const, state: draft.state, questions: draft.questions.map(questionInput) };
  }
  return {
    ...common,
    kind: "prompt" as const,
    messages: draft.messages.filter((m) => m.role !== "system" || m.content.trim() !== ""),
    score: {
      ...draft.score,
      range_min: data_type === ScoreType.Number ? draft.score.range_min : null,
      range_max: data_type === ScoreType.Number ? draft.score.range_max : null,
      low_means: data_type === ScoreType.Category ? "" : draft.score.low_means,
      high_means: data_type === ScoreType.Category ? "" : draft.score.high_means,
      categories: data_type === ScoreType.Category ? draft.score.categories : [],
      multiple: data_type === ScoreType.Category && draft.score.multiple,
    },
    reasoning: draft.reasoning,
    media: draft.media,
    temperature: temperature !== null && Number.isFinite(temperature) ? temperature : null,
  };
}

/** What an evaluator gives, in a few words: its score, or its questions. */
function givesWords(evaluator: Evaluator): string {
  if (evaluator.score) return scoreWords(evaluator.score);
  const names = (evaluator.questions ?? []).map((q) => q.name);
  return `typed: ${names.join(", ")}`;
}

/** What an evaluator's score is, in a few words. */
function scoreWords(score: ScoreSpec): string {
  if (score.data_type === ScoreType.Number) return `${score.name}, ${score.range_min} to ${score.range_max}`;
  if (score.data_type === ScoreType.Boolean) return `${score.name}, yes or no`;
  return `${score.name}, ${score.categories.map((c) => c.name).join(" / ")}`;
}

/** The project's evaluators, each its newest version, with the form that
 *  makes and changes one, tried on a trace before it is saved. */
export function Evaluators({
  projectId,
  evaluators,
  templates,
  typedTemplates = [],
  connections,
  rulesOf = {},
  contentKept,
  canEdit,
  here,
  settingsHref,
  connectionsHref,
}: {
  projectId: string;
  evaluators: readonly Evaluator[];
  templates: readonly EvaluatorTemplate[];
  typedTemplates?: readonly TypedTemplate[];
  connections: readonly ModelConnection[];
  /** The names of the rules each evaluator runs under, by its id. */
  rulesOf?: Readonly<Record<string, readonly string[]>>;
  contentKept: boolean;
  canEdit: boolean;
  /** The page's own path, which `?evaluator=` opens one evaluator's evaluations and versions under. */
  here: string;
  settingsHref: string;
  connectionsHref: string;
}) {
  const [editing, setEditing] = useState<Draft | null>(null);
  const [picking, setPicking] = useState(false);
  const { pending, run } = useAction();
  const canCreate = canEdit && contentKept && connections.length > 0;
  const connectionName = (id: string | null) =>
    id === null
      ? "the default connection"
      : (connections.find((c) => c.id === id)?.name ?? "a deleted connection");

  return (
    <>
      <PageHeader
        title="Evaluators"
        info="LLM-as-a-judge: a prompt and the score it gives, or typed questions each answered as a score, and the model on this project's own key that judges."
        action={
          canCreate ? (
            <PageAction primary="New evaluator" onClick={() => setPicking(true)}>
              <Plus aria-hidden />
              New evaluator
            </PageAction>
          ) : undefined
        }
      />
      <div className="grid gap-gutter text-sm">
        {!contentKept && (
          <NoticeCard
            title="This project keeps no content."
            body={
              <>
                An evaluator reads the prompts and answers it judges, and this project keeps none, so
                none can run here. Content is switched on in the project&apos;s{" "}
                <Link className="underline" href={settingsHref}>
                  Settings
                </Link>
                .
              </>
            }
          />
        )}
        {contentKept && connections.length === 0 && (
          <NoticeCard
            title="No model is connected."
            body={
              <>
                Evaluators judge with a model on this project&apos;s own key: connect one on its{" "}
                <Link className="underline" href={connectionsHref}>
                  Models
                </Link>{" "}
                page first.
              </>
            }
          />
        )}
        {picking && !editing && (
          <TemplatePicker
            templates={templates}
            typedTemplates={typedTemplates}
            onPick={(draft) => {
              setPicking(false);
              setEditing(draft);
            }}
            onCancel={() => setPicking(false)}
          />
        )}
        {editing && (
          <EvaluatorForm
            key={editing.id ?? editing.name}
            projectId={projectId}
            initial={editing}
            connections={connections}
            onDone={() => setEditing(null)}
          />
        )}
        {evaluators.length === 0 && contentKept && connections.length > 0 && !editing && !picking && (
          <NoticeCard
            title="No evaluators yet."
            body={canEdit ? "Start from a template or a blank prompt." : "Evaluators appear here once an owner or admin makes one."}
          />
        )}
        {evaluators.length > 0 && (
          <WholeListTable
            rows={evaluators}
            rowKey={(evaluator) => evaluator.id}
            defaultSort="name"
            searchLabel="Search evaluators"
            searchOf={(evaluator) => `${evaluator.name} ${evaluator.model ?? ""} ${(rulesOf[evaluator.id] ?? []).join(" ")}`}
            columns={[
              {
                key: "name",
                label: "Name",
                sort: { by: (evaluator) => evaluator.name.toLowerCase(), first: SortDirection.Asc },
                className: "font-medium whitespace-nowrap",
                cell: (evaluator) => (
                  <span className="flex items-center gap-gutter">
                    {evaluator.name}
                    <Badge variant="secondary">v{evaluator.version}</Badge>
                  </span>
                ),
              },
              { key: "gives", label: "Gives", className: "text-muted-foreground", cell: givesWords },
              {
                key: "model",
                label: "Model",
                className: "text-muted-foreground",
                cell: (evaluator) =>
                  `${evaluator.model ?? "its connection's first model"} on ${connectionName(evaluator.connection_id)}`,
              },
              {
                key: "rules",
                label: "Rules",
                className: "text-muted-foreground",
                cell: (evaluator) => (rulesOf[evaluator.id] ?? []).join(", ") || "None",
              },
              {
                key: "created",
                label: "Created",
                sort: { by: (evaluator) => evaluator.created_at, first: SortDirection.Desc },
                className: "text-xs text-muted-foreground whitespace-nowrap",
                cell: (evaluator) => <LocalTime iso={evaluator.created_at} mode="date" />,
              },
            ]}
            actions={(evaluator) => (
              <div className="flex justify-end gap-gutter">
                <Button variant="outline" size="sm" asChild>
                  <Link href={`${here}?evaluator=${evaluator.id}`}>
                    Evaluations{(evaluator.versions ?? 1) > 1 ? ` · ${evaluator.versions} versions` : ""}
                  </Link>
                </Button>
                {canEdit && (
                  <>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={!contentKept}
                      onClick={() => {
                        setPicking(false);
                        setEditing(draftOf(evaluator));
                      }}
                    >
                      Edit
                    </Button>
                    <ConfirmDialog
                      trigger={
                        <Button variant="outline" size="sm" disabled={pending}>
                          Delete
                        </Button>
                      }
                      title={`Delete ${evaluator.name}?`}
                      description="Every version goes with it. The scores it gave stay, each naming its version."
                      confirmLabel="Delete"
                      destructive
                      onConfirm={() =>
                        run(
                          () => deleteEvaluatorAction(projectId, evaluator.id),
                          () => toast.success(`${evaluator.name} deleted`),
                        )
                      }
                    />
                  </>
                )}
              </div>
            )}
          />
        )}
      </div>
    </>
  );
}

/** The library, by group, prompt and typed, and a blank of each: each
 *  copied, never linked. */
function TemplatePicker({
  templates,
  typedTemplates,
  onPick,
  onCancel,
}: {
  templates: readonly EvaluatorTemplate[];
  typedTemplates: readonly TypedTemplate[];
  onPick: (draft: Draft) => void;
  onCancel: () => void;
}) {
  const groups = [...new Set(templates.map((t) => t.group))];
  return (
    <Card className="grid gap-gutter p-gutter" role="region" aria-label="Start an evaluator">
      <div className="flex items-center justify-between gap-gutter">
        <p className="font-medium">Start from a template, or a blank one</p>
        <div className="flex gap-gutter">
          <Button variant="outline" size="sm" onClick={() => onPick({ ...BLANK, messages: BLANK.messages.map((m) => ({ ...m })) })}>
            Blank prompt
          </Button>
          <Button variant="outline" size="sm" onClick={() => onPick({ ...BLANK_TYPED, questions: BLANK_TYPED.questions.map(copyOf) })}>
            Blank typed
          </Button>
          <Button variant="outline" size="sm" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
      {groups.map((group) => (
        <div key={group} className="grid gap-2">
          <p className="text-xs font-medium text-muted-foreground">{GROUP_LABEL[group] ?? group}</p>
          <div className="grid gap-page sm:grid-cols-2 lg:grid-cols-3">
            {templates
              .filter((t) => t.group === group)
              .map((template) => (
                <button
                  key={template.key}
                  type="button"
                  className="grid gap-1 rounded-md border border-border p-gutter text-left hover:bg-muted"
                  onClick={() => onPick(draftOf(template))}
                >
                  <span className="font-medium">{template.name}</span>
                  <span className="text-muted-foreground">{template.description}</span>
                </button>
              ))}
          </div>
        </div>
      ))}
      {typedTemplates.length > 0 && (
        <div className="grid gap-2">
          <p className="text-xs font-medium text-muted-foreground">Typed, on a System One model</p>
          <div className="grid gap-page sm:grid-cols-2 lg:grid-cols-3">
            {typedTemplates.map((template) => (
              <button
                key={template.key}
                type="button"
                className="grid gap-1 rounded-md border border-border p-gutter text-left hover:bg-muted"
                onClick={() => onPick(draftOf(template))}
              >
                <span className="font-medium">{template.name}</span>
                <span className="text-muted-foreground">{template.description}</span>
              </button>
            ))}
          </div>
        </div>
      )}
    </Card>
  );
}

/** One evaluator's form: its prompt, its score, its model; tried on a trace,
 *  then saved as a new version. */
function EvaluatorForm({
  projectId,
  initial,
  connections,
  onDone,
}: {
  projectId: string;
  initial: Draft;
  connections: readonly ModelConnection[];
  onDone: () => void;
}) {
  const id = useId();
  const save = useAction();
  const trial = useAction();
  const [draft, setDraft] = useState<Draft>(initial);
  const [runId, setRunId] = useState("");
  const [tried, setTried] = useState<Tried | null>(null);
  const set = (change: Partial<Draft>) => setDraft((d) => ({ ...d, ...change }));
  const setScore = (change: Partial<ScoreSpec>) => setDraft((d) => ({ ...d, score: { ...d.score, ...change } }));
  const setMessage = (at: number, change: Partial<PromptMessage>) =>
    setDraft((d) => ({ ...d, messages: d.messages.map((m, i) => (i === at ? { ...m, ...change } : m)) }));
  const { score } = draft;
  const typed = draft.kind === "typed";
  const ready =
    draft.name.trim() !== "" &&
    (typed
      ? draft.state.trim() !== "" && draft.questions.length > 0 && draft.questions.every(complete)
      : score.name.trim() !== "" && draft.messages.some((m) => m.role === "user" && m.content.trim()));
  // A typed evaluator judges on a System One connection, a prompt on any
  // other; a deleted connection stays named until another is chosen, and
  // saving refuses it.
  const fits = fitting(connections, typed);
  const preset = connections.find((c) => c.is_default);
  const presetWords = !preset
    ? "The project's default"
    : fits.includes(preset)
      ? `The project's default, ${preset.name}`
      : `The project's default, ${preset.name}, which judges no ${typed ? "typed" : "prompt"} evaluator`;
  const connectionOptions = [
    "",
    ...fits.map((c) => c.id),
    ...(draft.connection_id && !connections.some((c) => c.id === draft.connection_id) ? [draft.connection_id] : []),
  ];
  return (
    <form
      aria-label={draft.id ? `Edit ${initial.name}` : "New evaluator"}
      className="grid gap-gutter rounded-md border border-border p-gutter"
      onSubmit={(event) => {
        event.preventDefault();
        if (!ready || save.pending) return;
        save.run(
          () => saveEvaluatorAction(projectId, inputOf(draft)),
          () => {
            toast.success(draft.id ? "A new version saved" : "Evaluator saved");
            onDone();
          },
        );
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor={`${id}name`}>Name</Label>
        <Input id={`${id}name`} value={draft.name} onChange={(e) => set({ name: e.target.value })} maxLength={EVALUATOR_NAME_MAX} />
      </div>

      {typed ? (
        <TypedFields id={id} draft={draft} set={set} />
      ) : (
        <>
          <fieldset className="grid gap-gutter">
            <legend className="mb-2 font-medium">Prompt</legend>
            <p className="text-muted-foreground">
              Variables are filled from what is judged:{" "}
              {VARIABLES.map(([name, what], at) => (
                <span key={name}>
                  <code className="font-mono text-xs">{`{{${name}}}`}</code> {what}
                  {at < VARIABLES.length - 1 ? "; " : "."}
                </span>
              ))}{" "}
              An assistant&apos;s message after a user&apos;s is a worked example.
            </p>
            {draft.messages.map((message, at) => (
              <div key={at} className="grid gap-2">
                <div className="flex items-center justify-between gap-gutter">
                  <Label htmlFor={`${id}message${at}`}>{ROLE_LABEL[message.role]}</Label>
                  {at > 1 && (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={() => setDraft((d) => ({ ...d, messages: d.messages.filter((_, i) => i !== at) }))}
                    >
                      Remove
                    </Button>
                  )}
                </div>
                <textarea
                  id={`${id}message${at}`}
                  value={message.content}
                  onChange={(e) => setMessage(at, { content: e.target.value })}
                  rows={message.role === "system" ? 4 : 6}
                  spellCheck={false}
                  placeholder={message.role === "system" ? "Who the judge is and what it looks for." : undefined}
                  className="rounded-md border border-input bg-transparent p-gutter font-mono text-xs"
                />
              </div>
            ))}
            <div className="flex gap-gutter">
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={draft.messages.length + 2 > EVALUATOR_MESSAGES_MAX}
                onClick={() =>
                  setDraft((d) => ({
                    ...d,
                    messages: [...d.messages, { role: "assistant", content: "" }, { role: "user", content: "" }],
                  }))
                }
              >
                Add a worked example
              </Button>
            </div>
          </fieldset>

          <fieldset className="grid gap-gutter">
            <legend className="mb-2 font-medium">Score</legend>
            <div className="grid gap-gutter sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor={`${id}score`}>Score name</Label>
                <Input id={`${id}score`} value={score.name} onChange={(e) => setScore({ name: e.target.value })} maxLength={SCORE_NAME_MAX} spellCheck={false} />
              </div>
              <div className="grid gap-2">
                <Label htmlFor={`${id}type`}>Type</Label>
                <EnumSelect
                  id={`${id}type`}
                  value={score.data_type}
                  options={Object.values(ScoreType)}
                  label={(each) => TYPE_LABEL[each]}
                  onChange={(data_type) =>
                    setScore({
                      data_type,
                      range_min: data_type === ScoreType.Number ? (score.range_min ?? 0) : null,
                      range_max: data_type === ScoreType.Number ? (score.range_max ?? 1) : null,
                      categories:
                        data_type === ScoreType.Category && score.categories.length === 0
                          ? [{ name: "", means: "" }, { name: "", means: "" }]
                          : score.categories,
                    })
                  }
                />
              </div>
            </div>
            {score.data_type === ScoreType.Number && (
              <div className="grid gap-gutter sm:grid-cols-2">
                <Ends
                  id={`${id}low`}
                  label="Least"
                  value={score.range_min}
                  means={score.low_means}
                  onValue={(range_min) => setScore({ range_min })}
                  onMeans={(low_means) => setScore({ low_means })}
                />
                <Ends
                  id={`${id}high`}
                  label="Most"
                  value={score.range_max}
                  means={score.high_means}
                  onValue={(range_max) => setScore({ range_max })}
                  onMeans={(high_means) => setScore({ high_means })}
                />
              </div>
            )}
            {score.data_type === ScoreType.Boolean && (
              <div className="grid gap-gutter sm:grid-cols-2">
                <div className="grid gap-2">
                  <Label htmlFor={`${id}yes`}>Yes means</Label>
                  <Input id={`${id}yes`} value={score.high_means} onChange={(e) => setScore({ high_means: e.target.value })} maxLength={EVALUATOR_MEANS_MAX} />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor={`${id}no`}>No means</Label>
                  <Input id={`${id}no`} value={score.low_means} onChange={(e) => setScore({ low_means: e.target.value })} maxLength={EVALUATOR_MEANS_MAX} />
                </div>
              </div>
            )}
            {score.data_type === ScoreType.Category && (
              <div className="grid gap-gutter">
                {score.categories.map((category, at) => (
                  <div key={at} className="grid gap-gutter sm:grid-cols-[12rem_1fr_auto] sm:items-end">
                    <div className="grid gap-2">
                      <Label htmlFor={`${id}cat${at}`}>Category</Label>
                      <Input
                        id={`${id}cat${at}`}
                        value={category.name}
                        maxLength={SCORE_CATEGORY_MAX}
                        spellCheck={false}
                        onChange={(e) =>
                          setScore({ categories: score.categories.map((c, i) => (i === at ? { ...c, name: e.target.value } : c)) })
                        }
                      />
                    </div>
                    <div className="grid gap-2">
                      <Label htmlFor={`${id}means${at}`}>Means</Label>
                      <Input
                        id={`${id}means${at}`}
                        value={category.means}
                        maxLength={EVALUATOR_MEANS_MAX}
                        onChange={(e) =>
                          setScore({ categories: score.categories.map((c, i) => (i === at ? { ...c, means: e.target.value } : c)) })
                        }
                      />
                    </div>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={score.categories.length <= 2}
                      onClick={() => setScore({ categories: score.categories.filter((_, i) => i !== at) })}
                    >
                      Remove
                    </Button>
                  </div>
                ))}
                <div className="flex flex-wrap items-center gap-gutter">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={score.categories.length >= SCORE_CATEGORIES_MAX}
                    onClick={() => setScore({ categories: [...score.categories, { name: "", means: "" }] })}
                  >
                    Add a category
                  </Button>
                  <label className="flex items-center gap-gutter">
                    <input type="checkbox" checked={score.multiple} onChange={(e) => setScore({ multiple: e.target.checked })} />
                    Several may apply at once
                  </label>
                </div>
              </div>
            )}
            <label className="flex items-center gap-gutter">
              <input type="checkbox" checked={draft.reasoning} onChange={(e) => set({ reasoning: e.target.checked })} />
              Ask for its reasoning first, kept as the score&apos;s comment
            </label>
            <label className="flex items-center gap-gutter">
              <input type="checkbox" checked={draft.media} onChange={(e) => set({ media: e.target.checked })} />
              Send the images, PDFs and recordings its variables name, as the model&apos;s own parts: up to five,
              10 MiB between them, each of a kind its provider takes; a model that reads text alone refuses them
            </label>
          </fieldset>
        </>
      )}

      <fieldset className={typed ? "grid gap-gutter sm:grid-cols-3" : "grid gap-gutter sm:grid-cols-4"}>
        <legend className="mb-2 font-medium">Model</legend>
        <div className="grid gap-2">
          <Label htmlFor={`${id}connection`}>Connection</Label>
          <EnumSelect
            id={`${id}connection`}
            value={draft.connection_id ?? ""}
            options={connectionOptions}
            label={(each) =>
              each === "" ? presetWords : (connections.find((c) => c.id === each)?.name ?? "A deleted connection")
            }
            onChange={(each) => set({ connection_id: each === "" ? null : each })}
            className="w-full"
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor={`${id}model`}>Model</Label>
          <Input
            id={`${id}model`}
            value={draft.model}
            onChange={(e) => set({ model: e.target.value })}
            placeholder="The connection's first"
            maxLength={MODEL_CONNECTION_MODEL_MAX}
            spellCheck={false}
          />
        </div>
        {!typed && (
          <div className="grid gap-2">
            <Label htmlFor={`${id}temperature`}>Temperature</Label>
            <Input
              id={`${id}temperature`}
              value={draft.temperature}
              onChange={(e) => set({ temperature: decimalTyped(e.target.value) })}
              placeholder="The provider's own"
              inputMode="decimal"
            />
          </div>
        )}
        <div className="grid gap-2">
          <Label htmlFor={`${id}weekly`}>Weekly spend limit, $</Label>
          <Input
            id={`${id}weekly`}
            value={draft.weeklyLimit}
            onChange={(e) => set({ weeklyLimit: decimalTyped(e.target.value) })}
            placeholder="None"
            inputMode="decimal"
          />
        </div>
      </fieldset>

      <fieldset className="grid gap-gutter">
        <legend className="mb-2 font-medium">Try it on a trace</legend>
        <p className="text-muted-foreground">
          A trace&apos;s run id, from its page. The try reads its content as you would, filed in the audit log,
          and keeps nothing.
        </p>
        <div className="flex flex-wrap gap-gutter">
          <Input
            aria-label="Run id"
            value={runId}
            onChange={(e) => setRunId(e.target.value)}
            placeholder="0193…"
            spellCheck={false}
            className="max-w-md font-mono text-xs"
          />
          <Button
            type="button"
            variant="outline"
            disabled={!ready || !runId.trim() || trial.pending}
            onClick={() =>
              trial.run(
                () => tryEvaluatorAction(projectId, inputOf(draft), runId),
                (done) => setTried(done.tried),
              )
            }
          >
            {trial.pending ? "Judging…" : "Try"}
          </Button>
        </div>
        {trial.error && (
          <p className="text-destructive" role="alert">
            {trial.error}
          </p>
        )}
        {tried && <TryResult tried={tried} questions={draft.questions} />}
      </fieldset>

      {save.error && (
        <p className="text-destructive" role="alert">
          {save.error}
        </p>
      )}
      <div className="flex gap-gutter">
        <Button type="submit" disabled={!ready || save.pending}>
          {save.pending ? "Asking the model…" : draft.id ? "Save a new version" : "Save"}
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

/** One end of a number's range: its value and what it means. */
function Ends({
  id,
  label,
  value,
  means,
  onValue,
  onMeans,
}: {
  id: string;
  label: string;
  value: number | null;
  means: string;
  onValue: (value: number | null) => void;
  onMeans: (means: string) => void;
}) {
  return (
    <div className="grid gap-2">
      <Label htmlFor={id}>{label}, and what it means</Label>
      <div className="flex gap-gutter">
        <Input
          id={id}
          className="w-20"
          inputMode="decimal"
          value={value ?? ""}
          onChange={(e) => {
            const n = Number(e.target.value);
            onValue(e.target.value.trim() === "" || !Number.isFinite(n) ? null : n);
          }}
        />
        <Input aria-label={`What the ${label.toLowerCase()} means`} value={means} onChange={(e) => onMeans(e.target.value)} maxLength={EVALUATOR_MEANS_MAX} />
      </div>
    </div>
  );
}

/** The variables a typed evaluator's state may name: what is judged, never corrections. */
const STATE_VARIABLES = VARIABLES.filter(([name]) => name !== "examples");

/** A typed evaluator's state and questions, each question its own score. */
function TypedFields({ id, draft, set }: { id: string; draft: Draft; set: (change: Partial<Draft>) => void }) {
  const setQuestion = (at: number, question: Question) =>
    set({ questions: draft.questions.map((q, i) => (i === at ? question : q)) });
  return (
    <>
      <fieldset className="grid gap-gutter">
        <legend className="mb-2 font-medium">State</legend>
        <p className="text-muted-foreground">
          What is judged, and nothing of how: the questions say that. A typed evaluator answers every question with
          its probabilities in one call, on TypeSafe&apos;s Jev or an open-weight model speaking its System One. It
          cannot abstain and gives no reasons, so an open question, or one needing written reasons, is a prompt
          evaluator&apos;s. Variables are filled from what is judged:{" "}
          {STATE_VARIABLES.map(([name, what], at) => (
            <span key={name}>
              <code className="font-mono text-xs">{`{{${name}}}`}</code> {what}
              {at < STATE_VARIABLES.length - 1 ? "; " : "."}
            </span>
          ))}
        </p>
        <textarea
          aria-label="State"
          value={draft.state}
          onChange={(e) => set({ state: e.target.value })}
          rows={6}
          spellCheck={false}
          className="rounded-md border border-input bg-transparent p-gutter font-mono text-xs"
        />
      </fieldset>
      <fieldset className="grid gap-gutter">
        <legend className="mb-2 font-medium">Questions</legend>
        {draft.questions.map((question, at) => (
          <QuestionFields
            key={at}
            id={`${id}q${at}`}
            question={question}
            onChange={(next) => setQuestion(at, next)}
            onRemove={draft.questions.length > 1 ? () => set({ questions: draft.questions.filter((_, i) => i !== at) }) : undefined}
          />
        ))}
        <div className="flex gap-gutter">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={draft.questions.length >= TYPED_QUESTIONS_MAX}
            onClick={() => set({ questions: [...draft.questions, blankQuestion("noul")] })}
          >
            Add a question
          </Button>
        </div>
      </fieldset>
    </>
  );
}

/** One typed question: its score's name, its type, its instructions, and
 *  what each answer it may give means. */
function QuestionFields({
  id,
  question,
  onChange,
  onRemove,
}: {
  id: string;
  question: Question;
  onChange: (question: Question) => void;
  onRemove?: () => void;
}) {
  const asked = { name: question.name, instructions: question.instructions };
  return (
    <div className="grid gap-gutter rounded-md border border-border p-gutter" role="group" aria-label={`Question ${question.name || "unnamed"}`}>
      <div className="grid gap-gutter sm:grid-cols-[1fr_12rem_auto] sm:items-end">
        <div className="grid gap-2">
          <Label htmlFor={`${id}name`}>Score name</Label>
          <Input
            id={`${id}name`}
            value={question.name}
            maxLength={SCORE_NAME_MAX}
            spellCheck={false}
            onChange={(e) => onChange({ ...question, name: e.target.value })}
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor={`${id}type`}>Answer</Label>
          <EnumSelect
            id={`${id}type`}
            value={question.type}
            options={["noul", "score", "choice"] as const}
            label={(each) => QUESTION_LABEL[each]}
            onChange={(type) => onChange({ ...blankQuestion(type), ...asked })}
          />
        </div>
        {onRemove && (
          <Button type="button" variant="outline" size="sm" onClick={onRemove}>
            Remove
          </Button>
        )}
      </div>
      <div className="grid gap-2">
        <Label htmlFor={`${id}instructions`}>Instructions</Label>
        <Input
          id={`${id}instructions`}
          value={question.instructions}
          maxLength={TYPED_INSTRUCTIONS_MAX}
          onChange={(e) => onChange({ ...question, instructions: e.target.value })}
        />
      </div>
      {question.type === "noul" && (
        <div className="grid gap-gutter sm:grid-cols-2">
          <p className="text-muted-foreground sm:col-span-2">What yes and no mean: both, or neither.</p>
          <div className="grid gap-2">
            <Label htmlFor={`${id}yes`}>Yes means</Label>
            <Input id={`${id}yes`} value={question.yes_means} maxLength={EVALUATOR_MEANS_MAX} onChange={(e) => onChange({ ...question, yes_means: e.target.value })} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor={`${id}no`}>No means</Label>
            <Input id={`${id}no`} value={question.no_means} maxLength={EVALUATOR_MEANS_MAX} onChange={(e) => onChange({ ...question, no_means: e.target.value })} />
          </div>
        </div>
      )}
      {question.type === "score" && (
        <div className="grid gap-gutter">
          {question.levels.map((level, at) => (
            <div key={at} className="grid gap-gutter sm:grid-cols-[1fr_auto] sm:items-end">
              <div className="grid gap-2">
                <Label htmlFor={`${id}level${at}`}>Level {at}{at === 0 ? ", the lowest" : ""}</Label>
                <Input
                  id={`${id}level${at}`}
                  value={level}
                  maxLength={EVALUATOR_MEANS_MAX}
                  onChange={(e) => onChange({ ...question, levels: question.levels.map((l, i) => (i === at ? e.target.value : l)) })}
                />
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={question.levels.length <= 2}
                onClick={() => onChange({ ...question, levels: question.levels.filter((_, i) => i !== at) })}
              >
                Remove
              </Button>
            </div>
          ))}
          <div className="flex gap-gutter">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={question.levels.length >= TYPED_LEVELS_MAX}
              onClick={() => onChange({ ...question, levels: [...question.levels, ""] })}
            >
              Add a level
            </Button>
          </div>
        </div>
      )}
      {question.type === "choice" && (
        <div className="grid gap-gutter">
          {question.options.map((option, at) => (
            <div key={at} className="grid gap-gutter sm:grid-cols-[12rem_1fr_auto] sm:items-end">
              <div className="grid gap-2">
                <Label htmlFor={`${id}option${at}`}>Option</Label>
                <Input
                  id={`${id}option${at}`}
                  value={option.name}
                  maxLength={SCORE_CATEGORY_MAX}
                  spellCheck={false}
                  onChange={(e) =>
                    onChange({ ...question, options: question.options.map((o, i) => (i === at ? { ...o, name: e.target.value } : o)) })
                  }
                />
              </div>
              <div className="grid gap-2">
                <Label htmlFor={`${id}optionMeans${at}`}>Means (optional)</Label>
                <Input
                  id={`${id}optionMeans${at}`}
                  value={option.means}
                  maxLength={EVALUATOR_MEANS_MAX}
                  onChange={(e) =>
                    onChange({ ...question, options: question.options.map((o, i) => (i === at ? { ...o, means: e.target.value } : o)) })
                  }
                />
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={question.options.length <= 2}
                onClick={() => onChange({ ...question, options: question.options.filter((_, i) => i !== at) })}
              >
                Remove
              </Button>
            </div>
          ))}
          <div className="flex gap-gutter">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={question.options.length >= TYPED_OPTIONS_MAX}
              onClick={() => onChange({ ...question, options: [...question.options, { name: "", means: "" }] })}
            >
              Add an option
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

/** A share, as a person reads it. */
function percent(p: number): string {
  return `${Math.round(p * 100)}%`;
}

/** A typed answer's probabilities, in words: yes's, or each level's, by its
 *  description in `levels` where given, or each option's. */
export function probabilityWords(answer: TypedAnswer, levels: readonly string[] = []): string {
  if (answer.noul !== undefined) return `yes ${percent(answer.noul)}`;
  const each = Object.entries(answer.probabilities ?? {})
    .map(([name, p]) => `${levels[Number(name)] ?? name} ${percent(p)}`)
    .join(", ");
  const confidence = answer.confidence != null ? `confidence ${percent(answer.confidence)}` : "";
  return [confidence, each].filter(Boolean).join(" · ");
}

/** What a judgement says, in a word or a number. */
function judgementWords(judgement: NonNullable<Tried["judgement"]>): string {
  if (judgement.categories.length > 0) return judgement.categories.join(", ");
  if (judgement.data_type === ScoreType.Boolean) return judgement.value === 1 ? "yes" : "no";
  return formatScoreValue(judgement.value ?? 0);
}

/** What a try gave: the score and its reasoning, or each typed question's
 *  score with its probabilities, or why the answer judged nothing; then
 *  what was sent. */
export function TryResult({ tried, questions = [] }: { tried: Tried; questions?: readonly Question[] }) {
  const answers = tried.answers ?? [];
  // A typed try sends a state and no system message.
  const typed = tried.system === "";
  const levelsOf = (name: string) => {
    const question = questions.find((q) => q.name.trim() === name);
    return question?.type === "score" ? question.levels : [];
  };
  return (
    <div className="grid gap-gutter rounded-md border border-border p-gutter" aria-label="The try's result" role="region">
      {answers.length > 0 ? (
        <ul className="grid gap-1">
          {answers.map((answered) => (
            <li key={answered.name}>
              <span className="font-medium">{answered.name}</span>: {judgementWords(answered)}{" "}
              <span className="text-muted-foreground">({probabilityWords(answered.answer, levelsOf(answered.name))})</span>
            </li>
          ))}
        </ul>
      ) : tried.judgement ? (
        <p>
          <span className="font-medium">{tried.judgement.name}</span>: {judgementWords(tried.judgement)}
        </p>
      ) : (
        <p className="text-destructive">{tried.refused}</p>
      )}
      {tried.reasoning && <p className="whitespace-pre-wrap text-muted-foreground">{tried.reasoning}</p>}
      <p className="text-xs text-muted-foreground">
        {tried.model} · {tried.input_tokens} tokens in, {tried.output_tokens} out
      </p>
      <details>
        <summary className="cursor-pointer">{typed ? "The state as sent" : "The prompt as sent"}</summary>
        <div className="mt-2 grid gap-gutter">
          {tried.system && <pre className="whitespace-pre-wrap font-mono text-xs">{tried.system}</pre>}
          {tried.messages.map((message, at) => (
            <div key={at} className="grid gap-1">
              <p className="text-xs font-medium text-muted-foreground">{ROLE_LABEL[message.role]}</p>
              <pre className="whitespace-pre-wrap font-mono text-xs">{message.content}</pre>
            </div>
          ))}
        </div>
      </details>
    </div>
  );
}
