// How a score reads wherever it is drawn: a badge on a trace, a column of the
// list, the head of a session.

import type { Score } from "@/lib/server/entities/scores";
import type { ScoreSummary } from "@/lib/types/traces";
import { ScoreSource, ScoreType } from "@/lib/types/enums";

/** A number to three figures at most, as a score is read. */
export function formatScoreValue(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(Math.abs(value) >= 10 ? 1 : 2);
}

/** Who gave a score, as a sentence ends: "relevance 0.8, by a member". */
export const SCORE_SOURCE: Record<Score["source"], string> = {
  [ScoreSource.Sdk]: "from your code",
  [ScoreSource.Member]: "by a member",
};

/** What one score says. */
export function scoreSaid(score: Score): string {
  switch (score.data_type) {
    case ScoreType.Boolean:
      return score.value === 1 ? "yes" : "no";
    case ScoreType.Number:
      return score.value == null ? "—" : formatScoreValue(score.value);
    case ScoreType.Category:
      return score.categories.join(", ");
  }
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

/** A row's scores in one line, as its cell says them on hover; nothing where it has none. */
export function scoresLine(scores: readonly ScoreSummary[]): string | undefined {
  return scores.map((score) => `${score.name}: ${scoreWords(score)}`).join(", ") || undefined;
}

/** What a summary's badge says on hover: how many scores it holds. */
export function scoreTitle(score: ScoreSummary): string {
  const of = score.count === 1 ? "1 score" : `${score.count} scores`;
  return score.data_type === ScoreType.Number && score.count > 1 ? `${score.name}: the mean of ${of}` : `${score.name}: ${of}`;
}

/** The longest comment a score keeps, the lane's. */
export const COMMENT_MAX = 4_000;

/** The most categories a category score holds, and the longest each is. */
export const SCORE_CATEGORIES_MAX = 10;
export const SCORE_CATEGORY_MAX = 64;

/** Where a score's comment is kept: under its trace's content, as a span the
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

/** Whether `id` is a span's own, 16 hex digits: not a score's comment, which
 *  the trace's content keeps under an id of its own. */
export function isSpanId(id: string): boolean {
  return /^[0-9a-f]{16}$/.test(id);
}
