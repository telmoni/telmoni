"use client";

import { createContext, useContext, useId, useState, type ReactNode } from "react";
import { MessageSquareText, PlusIcon, XIcon } from "lucide-react";

import { deleteScoreAction, putScoreAction } from "@/app/(app)/[organization]/[project]/traces/actions";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { Score } from "@/lib/server/entities/scores";
import { COMMENT_MAX, SCORE_SOURCE, scoreSaid } from "@/lib/scores";
import { ScoreSource, ScoreTarget, ScoreType } from "@/lib/types/enums";
import { useToastAction } from "@/lib/use-action";

// A trace's scores on its line: its own, then each span's by the span's name,
// each saying who gave it; a member's own taken back at its ×, an owner or
// admin any member's; one with a comment opens to it, read with the trace's
// content. Score adds one of the trace, as any member of the project may; the
// server holds every rule a score keeps, its refusal the toast.

export function TraceScores({
  projectId,
  traceId,
  scores,
  comments,
  spanNames,
  me,
  administers,
  keepsContent,
  onChanged,
}: {
  projectId: string;
  traceId: string;
  /** After a score is given or taken back: the peek reads its trace again,
   *  as the trace's page is drawn again by the action's refresh. */
  onChanged?: () => void;
  /** Whether the project keeps content, so a comment may be given. */
  keepsContent: boolean;
  scores: readonly Score[];
  /** Each score's comment by its id, from the trace's content as it was opened. */
  comments: Readonly<Record<string, string>>;
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
      {scores.map((score) => (
        <ScoreBadge
          key={score.score_id}
          score={score}
          span={score.span_id ? spanNames[score.span_id] ?? "a span" : null}
          comment={comments[score.score_id]}
          takeBack={
            score.source === ScoreSource.Member && (score.scorer === me || administers)
              ? () => {
                  if (!pending) run(() => deleteScoreAction(projectId, score.score_id), () => onChanged?.());
                }
              : undefined
          }
          pending={pending}
        />
      ))}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button variant="ghost" size="sm" className="text-muted-foreground hover:text-foreground">
            <PlusIcon />
            Score
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" aria-label="Score this trace" className="w-64">
          <ScoreForm
            pending={pending}
            keepsContent={keepsContent}
            onScore={(score) =>
              run(() => putScoreAction(projectId, { trace_id: traceId, ...score }), () => {
                setOpen(false);
                onChanged?.();
              })
            }
          />
        </PopoverContent>
      </Popover>
    </>
  );
}

/** One score's badge: its span where it is a span's, its name and value, who
 *  gave it on hover, its comment where it has one, and its × where the person
 *  may take it back. */
function ScoreBadge({
  score,
  span,
  comment,
  takeBack,
  pending = false,
}: {
  score: Score;
  span: string | null;
  comment: string | undefined;
  takeBack?: () => void;
  pending?: boolean;
}) {
  return (
    <Badge
      variant="outline"
      title={`${score.name}${span ? ` of ${span}` : ""}, ${SCORE_SOURCE[score.source]}`}
    >
      {span && <span className="text-muted-foreground">{span} · </span>}
      {score.name} <span className="tabular-nums text-muted-foreground">{scoreSaid(score)}</span>
      {comment && <Comment name={score.name} text={comment} />}
      {takeBack && (
        <button
          type="button"
          aria-label={`Take back score ${score.name}`}
          aria-disabled={pending || undefined}
          onClick={takeBack}
          className="inline-flex size-4 cursor-pointer items-center justify-center rounded-sm text-muted-foreground hover:text-foreground aria-disabled:cursor-not-allowed"
        >
          <XIcon className="size-icon" />
        </button>
      )}
    </Badge>
  );
}

/** A trace's scores and their comments, for the spans drawn under its line:
 *  none in a peek, which draws no scores. */
const TraceScoresContext = createContext<{
  scores: readonly Score[];
  comments: Readonly<Record<string, string>>;
} | null>(null);

export function TraceScoresProvider({
  scores,
  comments,
  children,
}: {
  scores: readonly Score[];
  comments: Readonly<Record<string, string>>;
  children: ReactNode;
}) {
  return <TraceScoresContext.Provider value={{ scores, comments }}>{children}</TraceScoresContext.Provider>;
}

/** The scores of one span beside it: read alone, since they are given and
 *  taken back on the trace's line. */
export function SpanScores({ spanId }: { spanId: string }) {
  const given = useContext(TraceScoresContext);
  const mine = (given?.scores ?? []).filter((score) => score.target === ScoreTarget.Span && score.span_id === spanId);
  if (!given || mine.length === 0) return null;
  return (
    <div aria-label="Its scores" role="group" className="flex flex-wrap items-center gap-1">
      {mine.map((score) => (
        <ScoreBadge key={score.score_id} score={score} span={null} comment={given.comments[score.score_id]} />
      ))}
    </div>
  );
}

/** A score's comment, opened from its badge. */
function Comment({ name, text }: { name: string; text: string }) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={`The comment on ${name}`}
          className="inline-flex size-4 cursor-pointer items-center justify-center rounded-sm text-muted-foreground hover:text-foreground"
        >
          <MessageSquareText className="size-icon" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" aria-label={`The comment on ${name}`} className="w-80">
        <p className="whitespace-pre-wrap break-words text-sm">{text}</p>
      </PopoverContent>
    </Popover>
  );
}

type Given = { name: string; data_type: ScoreType; value?: number; categories?: string[]; comment?: string };

/** A score's name, its type, and its value as the type takes it. */
function ScoreForm({
  pending,
  keepsContent,
  onScore,
}: {
  pending: boolean;
  keepsContent: boolean;
  onScore: (score: Given) => void;
}) {
  const id = useId();
  const [name, setName] = useState("");
  const [type, setType] = useState<ScoreType>(ScoreType.Number);
  const [value, setValue] = useState("");
  const [yes, setYes] = useState(true);
  const [comment, setComment] = useState("");
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
  const remark = comment.trim();
  return (
    <form
      className="grid gap-gutter"
      onSubmit={(event) => {
        event.preventDefault();
        if (given) onScore(remark ? { ...given, comment: remark } : given);
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
      {keepsContent && (
        <div className="grid gap-2">
          <Label htmlFor={`${id}comment`}>Comment</Label>
          <textarea
            id={`${id}comment`}
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            rows={3}
            maxLength={COMMENT_MAX}
            aria-describedby={`${id}comment-hint`}
            className="rounded-md border border-input bg-transparent px-gutter py-2 text-sm"
          />
          <p id={`${id}comment-hint`} className="text-xs text-muted-foreground">
            Optional. Content, kept as this trace&rsquo;s is, and read with it.
          </p>
        </div>
      )}
      <Button type="submit" variant="filled" disabled={pending || !given}>
        Score
      </Button>
    </form>
  );
}
