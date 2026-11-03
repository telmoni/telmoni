// How a score reads wherever it is drawn: a badge on a trace, a column of the
// list, the head of a session.

import type { ScoreSummary } from "@/lib/types/traces";
import { ScoreType } from "@/lib/types/enums";

/** A number to three figures at most, as a score is read. */
export function formatScoreValue(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(Math.abs(value) >= 10 ? 1 : 2);
}

/** A score name's value: a yes-or-no's share of yeses, a number's mean, a category's values. */
export function scoreWords(score: ScoreSummary): string {
  switch (score.data_type) {
    case ScoreType.Boolean:
      if (score.mean === null) return "—";
      return score.count === 1 ? (score.mean >= 0.5 ? "yes" : "no") : `${Math.round(score.mean * 100)}% yes`;
    case ScoreType.Number:
      return score.mean === null ? "—" : formatScoreValue(score.mean);
    case ScoreType.Category:
      return score.categories.join(", ") || "—";
  }
}

/** What a summary's badge says on hover: how many scores it holds. */
export function scoreTitle(score: ScoreSummary): string {
  const of = score.count === 1 ? "1 score" : `${score.count} scores`;
  return score.data_type === ScoreType.Number && score.count > 1 ? `${score.name}: the mean of ${of}` : `${score.name}: ${of}`;
}

/** The longest comment a score keeps, the lane's. */
export const COMMENT_MAX = 4_000;

/** Where a score's comment is kept: under its run's content, as a span the
 *  score's id names, which no span's 16 hex digits can be. */
const COMMENT_SPAN = "score/";
const COMMENT_FIELD = "telmoni.score.comment";

/** Each score's comment, by the score's id, from a trace's content as it was
 *  opened, so read and filed as the trace's content is; none where the
 *  content was not opened. */
export function commentsOf(
  content: readonly { span_id: string; fields: Readonly<Record<string, string>> }[] | null | undefined,
): Readonly<Record<string, string>> {
  const comments: Record<string, string> = {};
  for (const span of content ?? []) {
    const text = span.fields[COMMENT_FIELD];
    if (span.span_id.startsWith(COMMENT_SPAN) && text) comments[span.span_id.slice(COMMENT_SPAN.length)] = text;
  }
  return comments;
}

/** Whether `id` is a span's own, 16 hex digits: not a score's comment or an
 *  evaluator's example, which the run's content keeps under ids of their own. */
export function isSpanId(id: string): boolean {
  return /^[0-9a-f]{16}$/.test(id);
}
