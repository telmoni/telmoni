"use client";

import { Plus } from "lucide-react";
import Link from "next/link";
import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EnumSelect } from "@/components/enum-select";
import { NoticeCard } from "@/components/notice-card";
import { PageAction } from "@/components/page-action";
import { PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type {
  Evaluator,
  EvaluatorTemplate,
  PromptMessage,
  PromptRole,
  ScoreSpec,
  Tried,
} from "@/lib/server/entities/evaluators";
import type { ModelConnection } from "@/lib/server/entities/model-connections";
import { formatScoreValue } from "@/lib/scores";
import { ScoreType } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import { deleteEvaluatorAction, saveEvaluatorAction, tryEvaluatorAction } from "./actions";

/** The variables a prompt may name, each with what fills it. */
export const VARIABLES: ReadonlyArray<readonly [string, string]> = [
  ["input", "a run's or a span's input"],
  ["output", "its output"],
  ["metadata", "its agent, status, models, tokens and tags"],
  ["tool_calls", "its tool calls and their arguments"],
  ["trajectory", "its tool calls with their results, in order"],
  ["messages", "a session's conversation"],
  ["exchanges", "a session's person-and-agent pairs"],
  ["first_and_last", "a session's first prompt and last answer"],
  ["examples", "members' corrections"],
];

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

/** A draft as the form holds it. */
export type Draft = {
  id?: string;
  name: string;
  messages: PromptMessage[];
  score: ScoreSpec;
  reasoning: boolean;
  connection_id: string | null;
  model: string;
  temperature: string;
  weeklyLimit: string;
};

const BLANK: Draft = {
  name: "",
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
  connection_id: null,
  model: "",
  temperature: "",
  weeklyLimit: "",
};

/** A template or a saved version, as a draft to change. */
export function draftOf(from: Evaluator | EvaluatorTemplate): Draft {
  const saved = "id" in from;
  return {
    id: saved ? from.id : undefined,
    name: from.name,
    messages: from.messages.map((m) => ({ ...m })),
    score: { ...from.score, categories: from.score.categories.map((c) => ({ ...c })) },
    reasoning: from.reasoning,
    connection_id: saved ? from.connection_id : null,
    model: saved ? (from.model ?? "") : "",
    temperature: saved && from.temperature !== null ? String(from.temperature) : "",
    weeklyLimit: saved && from.weekly_limit !== null ? String(from.weekly_limit) : "",
  };
}

/** The draft as the lanes take it: a score's fields its type does not use left out. */
export function inputOf(draft: Draft) {
  const { data_type } = draft.score;
  const temperature = draft.temperature.trim() === "" ? null : Number(draft.temperature);
  const weekly = draft.weeklyLimit.trim() === "" ? null : Number(draft.weeklyLimit);
  return {
    id: draft.id,
    name: draft.name.trim(),
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
    connection_id: draft.connection_id,
    model: draft.model.trim() || null,
    temperature: temperature !== null && Number.isFinite(temperature) ? temperature : null,
    weekly_limit: weekly !== null && Number.isFinite(weekly) && weekly > 0 ? weekly : null,
  };
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
  connections,
  rulesOf = {},
  contentKept,
  canEdit,
  here,
  privacyHref,
  settingsHref,
}: {
  projectId: string;
  evaluators: readonly Evaluator[];
  templates: readonly EvaluatorTemplate[];
  connections: readonly ModelConnection[];
  /** The names of the rules each evaluator runs under, by its id. */
  rulesOf?: Readonly<Record<string, readonly string[]>>;
  contentKept: boolean;
  canEdit: boolean;
  /** The page's own path, which `?evaluator=` opens one evaluator's evaluations and versions under. */
  here: string;
  privacyHref: string;
  settingsHref: string;
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
        info="LLM-as-a-judge: a prompt, the score it gives, and the model on this project's own key that judges."
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
                ; the{" "}
                <Link className="underline" href={privacyHref}>
                  Privacy
                </Link>{" "}
                page says what it would keep.
              </>
            }
          />
        )}
        {contentKept && connections.length === 0 && (
          <NoticeCard
            title="No model is connected."
            body={
              <>
                Evaluators judge with a model on this project&apos;s own key: connect one on the
                project&apos;s{" "}
                <Link className="underline" href={settingsHref}>
                  Settings
                </Link>{" "}
                page first.
              </>
            }
          />
        )}
        {picking && !editing && (
          <TemplatePicker
            templates={templates}
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
          <Card className="gap-0 p-0">
            <ul className="divide-y divide-border">
              {evaluators.map((evaluator) => (
                <li key={evaluator.id} className="flex flex-wrap items-center justify-between gap-gutter p-gutter">
                  <div className="grid min-w-0">
                    <span className="flex items-center gap-gutter font-medium">
                      {evaluator.name}
                      <Badge variant="secondary">v{evaluator.version}</Badge>
                    </span>
                    <span className="truncate text-muted-foreground">
                      {scoreWords(evaluator.score)} · {evaluator.model ?? "its connection's first model"} on{" "}
                      {connectionName(evaluator.connection_id)} ·{" "}
                      {(rulesOf[evaluator.id] ?? []).length === 0
                        ? "under no rule"
                        : `under ${(rulesOf[evaluator.id] ?? []).join(", ")}`}
                    </span>
                  </div>
                  <div className="flex gap-gutter">
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
                </li>
              ))}
            </ul>
          </Card>
        )}
      </div>
    </>
  );
}

/** The library, by group, and a blank prompt: each copied, never linked. */
function TemplatePicker({
  templates,
  onPick,
  onCancel,
}: {
  templates: readonly EvaluatorTemplate[];
  onPick: (draft: Draft) => void;
  onCancel: () => void;
}) {
  const groups = [...new Set(templates.map((t) => t.group))];
  return (
    <Card className="grid gap-gutter p-gutter" role="region" aria-label="Start an evaluator">
      <div className="flex items-center justify-between gap-gutter">
        <p className="font-medium">Start from a template, or a blank prompt</p>
        <div className="flex gap-gutter">
          <Button variant="outline" size="sm" onClick={() => onPick({ ...BLANK, messages: BLANK.messages.map((m) => ({ ...m })) })}>
            Blank
          </Button>
          <Button variant="outline" size="sm" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
      {groups.map((group) => (
        <div key={group} className="grid gap-2">
          <p className="text-xs font-medium text-muted-foreground">{GROUP_LABEL[group] ?? group}</p>
          <div className="grid gap-gutter sm:grid-cols-2 lg:grid-cols-3">
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
  const ready = draft.name.trim() !== "" && score.name.trim() !== "" && draft.messages.some((m) => m.role === "user" && m.content.trim());
  // A deleted connection stays named until another is chosen; saving refuses it.
  const connectionOptions = [
    "",
    ...connections.map((c) => c.id),
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
        <Input id={`${id}name`} value={draft.name} onChange={(e) => set({ name: e.target.value })} maxLength={60} />
      </div>

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
            disabled={draft.messages.length > 18}
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
            <Input id={`${id}score`} value={score.name} onChange={(e) => setScore({ name: e.target.value })} maxLength={64} spellCheck={false} />
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
              <Input id={`${id}yes`} value={score.high_means} onChange={(e) => setScore({ high_means: e.target.value })} maxLength={500} />
            </div>
            <div className="grid gap-2">
              <Label htmlFor={`${id}no`}>No means</Label>
              <Input id={`${id}no`} value={score.low_means} onChange={(e) => setScore({ low_means: e.target.value })} maxLength={500} />
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
                    maxLength={64}
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
                    maxLength={500}
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
                disabled={score.categories.length >= 10}
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
      </fieldset>

      <fieldset className="grid gap-gutter sm:grid-cols-4">
        <legend className="mb-2 font-medium">Model</legend>
        <div className="grid gap-2">
          <Label htmlFor={`${id}connection`}>Connection</Label>
          <EnumSelect
            id={`${id}connection`}
            value={draft.connection_id ?? ""}
            options={connectionOptions}
            label={(each) =>
              each === "" ? "The project's default" : (connections.find((c) => c.id === each)?.name ?? "A deleted connection")
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
            maxLength={128}
            spellCheck={false}
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor={`${id}temperature`}>Temperature</Label>
          <Input
            id={`${id}temperature`}
            value={draft.temperature}
            onChange={(e) => set({ temperature: e.target.value })}
            placeholder="The provider's own"
            inputMode="decimal"
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor={`${id}weekly`}>Weekly spend limit, $</Label>
          <Input
            id={`${id}weekly`}
            value={draft.weeklyLimit}
            onChange={(e) => set({ weeklyLimit: e.target.value })}
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
        {tried && <TryResult tried={tried} />}
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
        <Input aria-label={`What the ${label.toLowerCase()} means`} value={means} onChange={(e) => onMeans(e.target.value)} maxLength={500} />
      </div>
    </div>
  );
}

/** What a try gave: the score and its reasoning, or why the answer judged
 *  nothing, then the prompt as it was sent. */
export function TryResult({ tried }: { tried: Tried }) {
  return (
    <div className="grid gap-gutter rounded-md border border-border p-gutter" aria-label="The try's result" role="region">
      {tried.judgement ? (
        <p>
          <span className="font-medium">{tried.judgement.name}</span>:{" "}
          {tried.judgement.categories.length > 0
            ? tried.judgement.categories.join(", ")
            : tried.judgement.data_type === ScoreType.Boolean
              ? tried.judgement.value === 1
                ? "yes"
                : "no"
              : formatScoreValue(tried.judgement.value ?? 0)}
        </p>
      ) : (
        <p className="text-destructive">{tried.refused}</p>
      )}
      {tried.reasoning && <p className="whitespace-pre-wrap text-muted-foreground">{tried.reasoning}</p>}
      <p className="text-xs text-muted-foreground">
        {tried.model} · {tried.input_tokens} tokens in, {tried.output_tokens} out
      </p>
      <details>
        <summary className="cursor-pointer">The prompt as sent</summary>
        <div className="mt-2 grid gap-gutter">
          <pre className="whitespace-pre-wrap font-mono text-xs">{tried.system}</pre>
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
