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
