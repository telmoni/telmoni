"use client";

import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EnumSelect } from "@/components/enum-select";
import { NoticeCard } from "@/components/notice-card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { EvaluationRule, Evaluator, RulePreview } from "@/lib/server/entities/evaluators";
import type { TraceView } from "@/lib/server/entities/traces";
import { formatCost } from "@/lib/traces";
import { EvaluationTarget, SpanKind } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import { VARIABLES } from "./_evaluators";
import { deleteRuleAction, previewRuleAction, saveRuleAction, saveSessionIdleAction } from "./actions";

const TARGET_LABEL: Record<EvaluationTarget, string> = {
  [EvaluationTarget.Run]: "Runs",
  [EvaluationTarget.Span]: "Spans",
  [EvaluationTarget.Session]: "Sessions",
};

const SPAN_KINDS = [SpanKind.ModelCall, SpanKind.ToolCall, SpanKind.Step, SpanKind.Span] as const;
type JudgedKind = (typeof SPAN_KINDS)[number];

const KIND_LABEL: Record<JudgedKind, string> = {
  [SpanKind.ModelCall]: "Model calls",
  [SpanKind.ToolCall]: "Tool calls",
  [SpanKind.Step]: "Steps",
  [SpanKind.Span]: "Other spans",
};

const LIST_ONLY = new Set(["sort", "dir", "days", "limit"]);

/** A rule as the form holds it. */
export type RuleDraft = {
  id?: string;
  name: string;
  target: EvaluationTarget;
  spanKind: JudgedKind;
  spanName: string;
  viewId: string;
  query: EvaluationRule["query"];
  percent: string;
  pointers: Record<string, string>;
  evaluators: string[];
  enabled: boolean;
  backfillFrom: string;
};

function draftOf(rule: EvaluationRule | null): RuleDraft {
  return {
    id: rule?.id,
    name: rule?.name ?? "",
    target: rule?.target ?? EvaluationTarget.Run,
    spanKind: (rule?.span_kind as JudgedKind | null) ?? SpanKind.ModelCall,
    spanName: rule?.span_name ?? "",
    viewId: "",
    query: rule?.query ?? {},
    percent: String(Math.round((rule?.sample_rate ?? 1) * 1000) / 10),
    pointers: { ...(rule?.pointers ?? {}) },
    evaluators: [...(rule?.evaluators ?? [])],
    enabled: rule?.enabled ?? true,
    backfillFrom: rule?.backfill_from?.slice(0, 10) ?? "",
  };
}

/** The draft as the lanes take it: a span's kind and name only on a span rule. */
export function ruleInputOf(draft: RuleDraft) {
  const percent = Number(draft.percent);
  const span = draft.target === EvaluationTarget.Span;
  return {
    id: draft.id,
    name: draft.name.trim(),
    target: draft.target,
    span_kind: span ? draft.spanKind : null,
    span_name: span && draft.spanName.trim() ? draft.spanName.trim() : null,
    query: draft.query,
    sample_rate: Number.isFinite(percent) ? Math.min(Math.max(percent, 0), 100) / 100 : 0,
    pointers: Object.fromEntries(Object.entries(draft.pointers).filter(([, p]) => p.trim() !== "")),
    evaluators: draft.evaluators,
    enabled: draft.enabled,
    backfill_from: draft.backfillFrom ? `${draft.backfillFrom}T00:00:00Z` : null,
  };
}

/** What a rule judges, in a few words. */
function judgedWords(rule: EvaluationRule): string {
  if (rule.target !== EvaluationTarget.Span) return TARGET_LABEL[rule.target];
  const kind = KIND_LABEL[rule.span_kind as JudgedKind] ?? "Spans";
  return rule.span_name ? `${kind} named ${rule.span_name}` : kind;
}

/** The project's rules — what its evaluators judge — with the form that
 *  makes one, previewed against the last seven days first, and the idle time
 *  a session is judged after. */
export function Rules({
  projectId,
  rules,
  evaluators,
  views,
  idleMinutes,
  canEdit,
  contentKept,
}: {
  projectId: string;
  rules: readonly EvaluationRule[];
  evaluators: readonly Evaluator[];
  views: readonly TraceView[];
  idleMinutes: number;
  canEdit: boolean;
  contentKept: boolean;
}) {
  const [editing, setEditing] = useState<RuleDraft | null>(null);
  const { pending, run } = useAction();
  const nameOf = (id: string) => evaluators.find((e) => e.id === id)?.name ?? "a deleted evaluator";
  return (
    <section className="grid gap-gutter text-sm" aria-labelledby="rules-heading">
      <div className="flex flex-wrap items-center justify-between gap-gutter">
        <h2 id="rules-heading" className="font-medium">
          Rules
        </h2>
        {canEdit && contentKept && evaluators.length > 0 && !editing && (
          <Button variant="outline" size="sm" onClick={() => setEditing(draftOf(null))}>
            New rule
          </Button>
        )}
      </div>
      <p className="text-muted-foreground">
        A rule says what its evaluators judge: the project&apos;s runs, its spans of a kind, or its sessions once
        quiet, under a saved view&apos;s filters, sampled.
      </p>
      {editing && (
        <RuleForm
          key={editing.id ?? "new"}
          projectId={projectId}
          initial={editing}
          evaluators={evaluators}
          views={views}
          onDone={() => setEditing(null)}
        />
      )}
      {rules.length === 0 && !editing && (
        <NoticeCard
          title="No rules yet."
          body={
            evaluators.length === 0
              ? "A rule runs evaluators; make one first."
              : "An evaluator judges nothing until a rule names it."
          }
        />
      )}
      {rules.length > 0 && (
        <Card className="gap-0 p-0">
          <ul className="divide-y divide-border">
            {rules.map((rule) => (
              <li key={rule.id} className="flex flex-wrap items-center justify-between gap-gutter p-gutter">
                <div className="grid min-w-0">
                  <span className="flex items-center gap-gutter font-medium">
                    {rule.name}
                    {!rule.enabled && <Badge variant="secondary">Off</Badge>}
                  </span>
                  <span className="truncate text-muted-foreground">
                    {judgedWords(rule)} · {Math.round(rule.sample_rate * 1000) / 10}% ·{" "}
                    {rule.evaluators.map(nameOf).join(", ")}
                  </span>
                  {rule.backfill_from && (
                    <span className="text-muted-foreground">
                      Reaching back to {rule.backfill_from.slice(0, 10)}:{" "}
                      {rule.backfill_done_at ? "done" : "under way"}, {rule.backfill_seen} looked over
                    </span>
                  )}
                </div>
                {canEdit && (
                  <div className="flex gap-gutter">
                    <Button variant="outline" size="sm" disabled={!contentKept} onClick={() => setEditing(draftOf(rule))}>
                      Edit
                    </Button>
                    <ConfirmDialog
                      trigger={
                        <Button variant="outline" size="sm" disabled={pending}>
                          Delete
                        </Button>
                      }
                      title={`Delete ${rule.name}?`}
                      description="Its evaluators stay, and judge nothing it named."
                      confirmLabel="Delete"
                      destructive
                      onConfirm={() =>
                        run(
                          () => deleteRuleAction(projectId, rule.id),
                          () => toast.success(`${rule.name} deleted`),
                        )
                      }
                    />
                  </div>
                )}
              </li>
            ))}
          </ul>
        </Card>
      )}
      <SessionIdle projectId={projectId} minutes={idleMinutes} canEdit={canEdit} />
    </section>
  );
}

/** How long a session is quiet before a rule judges it. */
function SessionIdle({ projectId, minutes, canEdit }: { projectId: string; minutes: number; canEdit: boolean }) {
  const id = useId();
  const [typed, setTyped] = useState(String(minutes));
  const { pending, error, run } = useAction();
  const n = Number(typed);
  const valid = Number.isInteger(n) && n >= 2 && n <= 1440;
  return (
    <form
      className="flex flex-wrap items-end gap-gutter"
      onSubmit={(event) => {
        event.preventDefault();
        if (!valid || pending) return;
        run(
          () => saveSessionIdleAction(projectId, n),
          () => toast.success("Idle time saved"),
        );
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor={`${id}idle`}>A session is judged once quiet for, in minutes</Label>
        <Input
          id={`${id}idle`}
          className="w-24"
          inputMode="numeric"
          value={typed}
          disabled={!canEdit}
          onChange={(e) => setTyped(e.target.value)}
        />
      </div>
      {canEdit && (
        <Button type="submit" variant="outline" size="sm" disabled={!valid || n === minutes || pending}>
          Save
        </Button>
      )}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

/** One rule's form, previewed against the last seven days before it is saved. */
function RuleForm({
  projectId,
  initial,
  evaluators,
  views,
  onDone,
}: {
  projectId: string;
  initial: RuleDraft;
  evaluators: readonly Evaluator[];
  views: readonly TraceView[];
  onDone: () => void;
}) {
  const id = useId();
  const save = useAction();
  const look = useAction();
  const [draft, setDraft] = useState<RuleDraft>(initial);
  const [preview, setPreview] = useState<RulePreview | null>(null);
  const set = (change: Partial<RuleDraft>) => {
    setPreview(null);
    setDraft((d) => ({ ...d, ...change }));
  };
  const percent = Number(draft.percent);
  const ready =
    draft.name.trim() !== "" && draft.evaluators.length > 0 && Number.isFinite(percent) && percent > 0 && percent <= 100;
  const viewOptions = ["", ...views.map((v) => v.id)];
  return (
    <form
      aria-label={draft.id ? `Edit ${initial.name}` : "New rule"}
      className="grid gap-gutter rounded-md border border-border p-gutter"
      onSubmit={(event) => {
        event.preventDefault();
        if (!ready || save.pending) return;
        save.run(
          () => saveRuleAction(projectId, ruleInputOf(draft)),
          () => {
            toast.success("Rule saved");
            onDone();
          },
        );
      }}
    >
      <div className="grid gap-gutter sm:grid-cols-2">
        <div className="grid gap-2">
          <Label htmlFor={`${id}name`}>Name</Label>
          <Input id={`${id}name`} value={draft.name} onChange={(e) => set({ name: e.target.value })} maxLength={60} />
        </div>
        <div className="grid gap-2">
          <Label htmlFor={`${id}target`}>Judges</Label>
          <EnumSelect
            id={`${id}target`}
            value={draft.target}
            options={Object.values(EvaluationTarget)}
            label={(each) => TARGET_LABEL[each]}
            onChange={(target) => set({ target })}
            className="w-full"
          />
        </div>
      </div>
      {draft.target === EvaluationTarget.Span && (
        <div className="grid gap-gutter sm:grid-cols-2">
          <div className="grid gap-2">
            <Label htmlFor={`${id}kind`}>Kind</Label>
            <EnumSelect
              id={`${id}kind`}
              value={draft.spanKind}
              options={SPAN_KINDS}
              label={(each) => KIND_LABEL[each]}
              onChange={(spanKind) => set({ spanKind })}
              className="w-full"
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor={`${id}span`}>Name, if one</Label>
            <Input id={`${id}span`} value={draft.spanName} onChange={(e) => set({ spanName: e.target.value })} spellCheck={false} />
          </div>
        </div>
      )}
      <div className="grid gap-gutter sm:grid-cols-2">
        <div className="grid gap-2">
          <Label htmlFor={`${id}view`}>Filters, from a saved view of the trace list</Label>
          <EnumSelect
            id={`${id}view`}
            value={draft.viewId}
            options={viewOptions}
            label={(each) =>
              each === ""
                ? Object.keys(draft.query).length > 0 && draft.id
                  ? "As saved"
                  : "None: every one"
                : (views.find((v) => v.id === each)?.name ?? each)
            }
            onChange={(viewId) => {
              const view = views.find((v) => v.id === viewId);
              // A rule takes the view's filters, never its sort, window or page.
              const filters = Object.fromEntries(
                Object.entries(view?.query ?? {}).filter(([key]) => !LIST_ONLY.has(key)),
              ) as RuleDraft["query"];
              set({ viewId, query: view ? filters : draft.id ? initial.query : {} });
            }}
            className="w-full"
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor={`${id}rate`}>Share judged, in percent</Label>
          <Input
            id={`${id}rate`}
            className="w-24"
            inputMode="decimal"
            value={draft.percent}
            onChange={(e) => set({ percent: e.target.value })}
          />
        </div>
      </div>
      <fieldset className="grid gap-2">
        <legend className="mb-2">Evaluators</legend>
        {evaluators.map((evaluator) => (
          <label key={evaluator.id} className="flex items-center gap-gutter">
            <input
              type="checkbox"
              checked={draft.evaluators.includes(evaluator.id)}
              onChange={(e) =>
                set({
                  evaluators: e.target.checked
                    ? [...draft.evaluators, evaluator.id]
                    : draft.evaluators.filter((each) => each !== evaluator.id),
                })
              }
            />
            {evaluator.name}
          </label>
        ))}
      </fieldset>
      <details>
        <summary className="cursor-pointer">Narrow a variable by a JSON Pointer</summary>
        <div className="mt-2 grid gap-gutter sm:grid-cols-2">
          {VARIABLES.filter(([name]) => name !== "examples").map(([name]) => (
            <div key={name} className="grid gap-2">
              <Label htmlFor={`${id}pointer-${name}`}>
                <code className="font-mono text-xs">{`{{${name}}}`}</code>
              </Label>
              <Input
                id={`${id}pointer-${name}`}
                value={draft.pointers[name] ?? ""}
                placeholder="/0/content"
                spellCheck={false}
                className="font-mono text-xs"
                onChange={(e) => set({ pointers: { ...draft.pointers, [name]: e.target.value } })}
              />
            </div>
          ))}
        </div>
      </details>
      <div className="grid gap-gutter sm:grid-cols-2">
        <div className="grid gap-2">
          <Label htmlFor={`${id}since`}>Also judge what arrived since, if a date</Label>
          <Input id={`${id}since`} type="date" value={draft.backfillFrom} onChange={(e) => set({ backfillFrom: e.target.value })} />
        </div>
        <label className="flex items-center gap-gutter self-end">
          <input type="checkbox" checked={draft.enabled} onChange={(e) => set({ enabled: e.target.checked })} />
          On
        </label>
      </div>
      <div className="flex flex-wrap items-center gap-gutter">
        <Button
          type="button"
          variant="outline"
          disabled={!ready || look.pending}
          onClick={() =>
            look.run(
              () => previewRuleAction(projectId, ruleInputOf(draft)),
              (done) => setPreview(done.preview),
            )
          }
        >
          {look.pending ? "Counting…" : "Preview the last 7 days"}
        </Button>
      </div>
      {look.error && (
        <p className="text-destructive" role="alert">
          {look.error}
        </p>
      )}
      {preview && (
        <div className="grid gap-1 rounded-md border border-border p-gutter" role="region" aria-label="Preview">
          <p>
            {preview.targets} found over the last {preview.days} days; {preview.judged} judged at this share.
          </p>
          {preview.evaluators.map((each) => (
            <p key={each.id} className="text-muted-foreground">
              {each.name} on {each.model}: {each.cost === null ? "no price for this model" : `about ${formatCost(each.cost)}`}
            </p>
          ))}
        </div>
      )}
      {save.error && (
        <p className="text-destructive" role="alert">
          {save.error}
        </p>
      )}
      <div className="flex gap-gutter">
        <Button type="submit" disabled={!ready || save.pending}>
          Save
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
