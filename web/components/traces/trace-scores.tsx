"use client";

import { useId, useState } from "react";
import { PlusIcon, XIcon } from "lucide-react";

import { deleteScoreAction, putScoreAction } from "@/app/(app)/[organization]/[project]/traces/actions";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { Score } from "@/lib/server/entities/scores";
import { formatScoreValue } from "@/lib/scores";
import { ScoreSource, ScoreType } from "@/lib/types/enums";
import { useToastAction } from "@/lib/use-action";

// A trace's scores on its line: the run's own, then each span's by the span's
// name, each saying who gave it; a member's own taken back at its ×, an owner
// or admin any member's. Score adds one of the run, as any member of the
// project may; the server holds every rule a score keeps, its refusal the toast.

const SOURCE: Record<Score["source"], string> = {
  [ScoreSource.Sdk]: "from your code",
  [ScoreSource.Member]: "by a member",
  [ScoreSource.Evaluator]: "by an evaluator",
};

/** What one score says. */
function said(score: Score): string {
  switch (score.data_type) {
    case ScoreType.Boolean:
      return score.value === 1 ? "yes" : "no";
    case ScoreType.Number:
      return score.value == null ? "—" : formatScoreValue(score.value);
    case ScoreType.Category:
      return score.categories.join(", ");
  }
}

export function TraceScores({
  projectId,
  runId,
  scores,
  spanNames,
  me,
  administers,
}: {
  projectId: string;
  runId: string;
  scores: readonly Score[];
  /** Each span's name by its id, for a span's score. */
  spanNames: Readonly<Record<string, string>>;
  /** The person's own user id: their scores are theirs to take back. */
  me: string;
  administers: boolean;
}) {
  const { pending, run } = useToastAction();
  const [open, setOpen] = useState(false);
  return (
    <>
      {scores.map((score) => {
        const span = score.span_id ? spanNames[score.span_id] ?? "a span" : null;
        const takeable = score.source === ScoreSource.Member && (score.scorer === me || administers);
        return (
          <Badge
            key={score.score_id}
            variant="outline"
            className="gap-0.5 pr-0.5 text-xs font-normal"
            title={`${score.name}${span ? ` of ${span}` : ""}, ${SOURCE[score.source]}`}
          >
            {span && <span className="text-muted-foreground">{span} · </span>}
            {score.name} <span className="tabular-nums text-muted-foreground">{said(score)}</span>
            {takeable && (
              <button
                type="button"
                aria-label={`Take back score ${score.name}`}
                aria-disabled={pending || undefined}
                onClick={() => {
                  if (!pending) run(() => deleteScoreAction(projectId, score.score_id), () => {});
                }}
                className="inline-flex size-4 cursor-pointer items-center justify-center rounded-sm text-muted-foreground outline-none hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset aria-disabled:cursor-not-allowed"
              >
                <XIcon className="size-3" />
              </button>
            )}
          </Badge>
        );
      })}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button variant="ghost" size="xs" className="text-muted-foreground hover:text-foreground">
            <PlusIcon />
            Score
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" aria-label="Score this trace" className="w-64 p-gutter">
          <ScoreForm
            pending={pending}
            onScore={(score) =>
              run(() => putScoreAction(projectId, { run_id: runId, ...score }), () => setOpen(false))
            }
          />
        </PopoverContent>
      </Popover>
    </>
  );
}

type Given = { name: string; data_type: ScoreType; value?: number; categories?: string[] };

/** A score's name, its type, and its value as the type takes it. */
function ScoreForm({ pending, onScore }: { pending: boolean; onScore: (score: Given) => void }) {
  const id = useId();
  const [name, setName] = useState("");
  const [type, setType] = useState<ScoreType>(ScoreType.Number);
  const [value, setValue] = useState("");
  const [yes, setYes] = useState(true);
  const number = Number(value);
  const given: Given | null =
    !name.trim()
      ? null
      : type === ScoreType.Number
        ? value.trim() && Number.isFinite(number)
          ? { name: name.trim(), data_type: type, value: number }
          : null
        : type === ScoreType.Boolean
          ? { name: name.trim(), data_type: type, value: yes ? 1 : 0 }
          : value.trim()
            ? {
                name: name.trim(),
                data_type: type,
                categories: value
                  .split(",")
                  .map((c) => c.trim())
                  .filter(Boolean),
              }
            : null;
  return (
    <form
      className="grid gap-gutter"
      onSubmit={(event) => {
        event.preventDefault();
        if (given) onScore(given);
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor={`${id}name`}>Name</Label>
        <Input id={`${id}name`} value={name} onChange={(e) => setName(e.target.value)} autoComplete="off" />
      </div>
      <fieldset className="grid gap-2">
        <legend className="mb-2 text-sm font-medium">Type</legend>
        <div className="flex flex-wrap gap-gutter text-sm">
          {(
            [
              [ScoreType.Number, "Number"],
              [ScoreType.Boolean, "Yes or no"],
              [ScoreType.Category, "Category"],
            ] as const
          ).map(([each, label]) => (
            <label key={each} className="flex items-center gap-1">
              <input type="radio" name={`${id}type`} checked={type === each} onChange={() => setType(each)} />
              {label}
            </label>
          ))}
        </div>
      </fieldset>
      {type === ScoreType.Boolean ? (
        <fieldset className="flex gap-gutter text-sm">
          <legend className="sr-only">Value</legend>
          <label className="flex items-center gap-1">
            <input type="radio" name={`${id}yes`} checked={yes} onChange={() => setYes(true)} />
            Yes
          </label>
          <label className="flex items-center gap-1">
            <input type="radio" name={`${id}yes`} checked={!yes} onChange={() => setYes(false)} />
            No
          </label>
        </fieldset>
      ) : (
        <div className="grid gap-2">
          <Label htmlFor={`${id}value`}>{type === ScoreType.Number ? "Value" : "Categories, by commas"}</Label>
          <Input
            id={`${id}value`}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            inputMode={type === ScoreType.Number ? "decimal" : undefined}
            autoComplete="off"
          />
        </div>
      )}
      <Button type="submit" disabled={pending || !given}>
        Score
      </Button>
    </form>
  );
}
