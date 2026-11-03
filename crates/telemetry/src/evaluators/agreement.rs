//! How far an evaluator agrees with the project's members, as Langfuse's
//! score analytics read it: each of its scores paired with a member's of the
//! same name on the same run, span or session, given over a window. A yes or
//! no and a category are read by the share agreeing, Cohen's kappa and the
//! confusion matrix; a number by the mean difference, the evaluator's less
//! the member's. Each score name is read apart, so a typed evaluator's
//! questions never meet in one matrix.

use clickhouse::Row;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use telmoni_shared::ScoreType;

use crate::store::{Projects, Store, text};

/// The longest window, in days, and the one read where none is asked.
pub const DAYS_MAX: u16 = 360;
pub const DAYS_DEFAULT: u16 = 30;

/// The most pairs one reading folds.
pub const PAIRS_MAX: u64 = 10_000;

/// Each of the evaluator's scores still standing, beside each member's of
/// the same name on the same target given in the window, the newest first;
/// a category's labels sorted and joined, so a set reads alike in any order.
const PAIRS_SQL: &str = "SELECT e.name AS name, e.data_type AS data_type, e.value AS judged, \
        arrayStringConcat(arraySort(e.categories), ', ') AS judged_label, \
        m.value AS given, arrayStringConcat(arraySort(m.categories), ', ') AS given_label \
     FROM (SELECT name, target, run_id, span_id, session_id, data_type, value, categories \
             FROM telemetry.scores FINAL \
            WHERE project_id IN {projects:Array(String)} AND source = 'evaluator' \
              AND scorer = {evaluator:String} AND is_deleted = 0) AS e \
     INNER JOIN (SELECT name, target, run_id, span_id, session_id, data_type, value, categories, \
                        received_at \
             FROM telemetry.scores FINAL \
            WHERE project_id IN {projects:Array(String)} AND source = 'member' AND is_deleted = 0 \
              AND received_at >= now64(6) - toIntervalDay({days:UInt16})) AS m \
     ON e.name = m.name AND e.target = m.target AND e.run_id = m.run_id \
        AND e.span_id = m.span_id AND e.session_id = m.session_id \
     WHERE e.data_type = m.data_type \
     ORDER BY m.received_at DESC \
     LIMIT {limit:UInt64}";

/// One evaluator's score and a member's of the same target and name.
#[derive(Debug, Clone, PartialEq, Row, Deserialize)]
pub struct Pair {
    pub name: String,
    #[serde(with = "text")]
    pub data_type: ScoreType,
    pub judged: f64,
    pub judged_label: String,
    pub given: f64,
    pub given_label: String,
}

/// What the agreement reads of each score name the evaluator gives, over `days`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agreements {
    pub days: u16,
    /// One a score name with a pair, by name.
    pub scores: Vec<Agreement>,
}

/// `pairs` read a score name at a time.
#[must_use]
pub fn agreements(days: u16, pairs: &[Pair]) -> Agreements {
    let mut by_name: std::collections::BTreeMap<&str, Vec<Pair>> =
        std::collections::BTreeMap::new();
    for pair in pairs {
        by_name
            .entry(pair.name.as_str())
            .or_default()
            .push(pair.clone());
    }
    Agreements {
        days,
        scores: by_name
            .into_iter()
            .map(|(name, pairs)| Agreement {
                name: name.to_owned(),
                ..agreement(days, &pairs)
            })
            .collect(),
    }
}

/// What the agreement reads of one score name, over `days`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agreement {
    /// The score's name.
    pub name: String,
    pub days: u16,
    /// The pairs' score type; none without a pair.
    pub data_type: Option<ScoreType>,
    pub pairs: u64,
    /// A yes or no's and a category's: the pairs that agree, and their share.
    pub agreeing: Option<u64>,
    pub share: Option<f64>,
    /// Cohen's kappa, none where chance alone would agree on every pair.
    pub kappa: Option<f64>,
    /// The confusion matrix's labels, and its rows, the evaluator's, by its
    /// columns, the members'.
    pub labels: Vec<String>,
    pub matrix: Vec<Vec<u64>>,
    /// A number's: the evaluator's less the member's, on average.
    pub mean_difference: Option<f64>,
}

/// `pairs` read: the type of most of them, the rest of another left out.
#[must_use]
pub fn agreement(days: u16, pairs: &[Pair]) -> Agreement {
    let mut of_type: [(ScoreType, usize); 3] = [
        (ScoreType::Boolean, 0),
        (ScoreType::Number, 0),
        (ScoreType::Category, 0),
    ];
    for pair in pairs {
        if let Some(slot) = of_type.iter_mut().find(|(t, _)| *t == pair.data_type) {
            slot.1 += 1;
        }
    }
    let (data_type, _) = of_type
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
        .unwrap_or((ScoreType::Number, 0));
    let pairs: Vec<&Pair> = pairs.iter().filter(|p| p.data_type == data_type).collect();
    let counted = u64::try_from(pairs.len()).unwrap_or(u64::MAX);
    let none = Agreement {
        name: String::new(),
        days,
        data_type: None,
        pairs: 0,
        agreeing: None,
        share: None,
        kappa: None,
        labels: Vec::new(),
        matrix: Vec::new(),
        mean_difference: None,
    };
    if pairs.is_empty() {
        return none;
    }
    #[expect(clippy::cast_precision_loss, reason = "a count of pairs is small")]
    let n = pairs.len() as f64;
    if data_type == ScoreType::Number {
        let difference = pairs.iter().map(|p| p.judged - p.given).sum::<f64>() / n;
        return Agreement {
            data_type: Some(data_type),
            pairs: counted,
            mean_difference: difference.is_finite().then_some(difference),
            ..none
        };
    }
    let label = |value: f64, label: &str| match data_type {
        ScoreType::Boolean => if value >= 0.5 { "yes" } else { "no" }.to_owned(),
        _ => label.to_owned(),
    };
    let mut labels: Vec<String> = match data_type {
        ScoreType::Boolean => vec!["yes".into(), "no".into()],
        _ => {
            let mut seen: Vec<String> = pairs
                .iter()
                .flat_map(|p| [p.judged_label.clone(), p.given_label.clone()])
                .collect();
            seen.sort();
            seen.dedup();
            seen
        }
    };
    labels.retain(|l| !l.is_empty() || data_type == ScoreType::Boolean);
    let at = |l: &str| labels.iter().position(|each| each == l);
    let mut matrix = vec![vec![0u64; labels.len()]; labels.len()];
    for pair in &pairs {
        let row = at(&label(pair.judged, &pair.judged_label));
        let column = at(&label(pair.given, &pair.given_label));
        if let Some(cell) = row
            .zip(column)
            .and_then(|(row, column)| matrix.get_mut(row)?.get_mut(column))
        {
            *cell += 1;
        }
    }
    let agreeing: u64 = matrix
        .iter()
        .enumerate()
        .filter_map(|(i, row)| row.get(i))
        .sum();
    let total: u64 = matrix.iter().flatten().sum();
    #[expect(clippy::cast_precision_loss, reason = "counts of pairs are small")]
    let (share, kappa) = if total == 0 {
        (None, None)
    } else {
        let total = total as f64;
        let observed = agreeing as f64 / total;
        let chance: f64 = (0..labels.len())
            .map(|i| {
                let row: u64 = matrix.get(i).map_or(0, |r| r.iter().sum());
                let column: u64 = matrix.iter().filter_map(|r| r.get(i)).sum();
                (row as f64 / total) * (column as f64 / total)
            })
            .sum();
        let kappa = (chance < 1.0).then(|| (observed - chance) / (1.0 - chance));
        (Some(observed), kappa)
    };
    Agreement {
        data_type: Some(data_type),
        // Those the matrix holds: a pair with no label reads as none.
        pairs: total,
        agreeing: Some(agreeing),
        share,
        kappa,
        labels,
        matrix,
        ..none
    }
}

/// The evaluator's pairs over `days`, read.
///
/// # Errors
///
/// ClickHouse did not answer.
pub async fn pairs(
    store: &Store,
    projects: &Projects,
    evaluator: Uuid,
    days: u16,
) -> clickhouse::error::Result<Vec<Pair>> {
    store
        .read(projects, PAIRS_SQL)
        .param("evaluator", evaluator.to_string())
        .param("days", days)
        .param("limit", PAIRS_MAX)
        .fetch_all()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(data_type: ScoreType, judged: f64, given: f64) -> Pair {
        Pair {
            name: "relevant".into(),
            data_type,
            judged,
            judged_label: String::new(),
            given,
            given_label: String::new(),
        }
    }

    /// A yes or no: 3 of 4 agree; chance agrees on half, so kappa is a half.
    #[test]
    fn a_yes_or_no_reads_its_share_its_kappa_and_its_matrix() {
        let b = ScoreType::Boolean;
        let read = agreement(
            30,
            &[
                pair(b, 1.0, 1.0),
                pair(b, 0.0, 0.0),
                pair(b, 1.0, 0.0),
                pair(b, 0.0, 0.0),
            ],
        );
        assert_eq!(read.pairs, 4);
        assert_eq!(read.agreeing, Some(3));
        assert_eq!(read.share, Some(0.75));
        assert_eq!(read.labels, ["yes", "no"]);
        assert_eq!(read.matrix, [[1, 1], [0, 2]]);
        // Rows yes 2, no 2; columns yes 1, no 3: chance 0.5*0.25 + 0.5*0.75 = 0.5.
        assert!(
            read.kappa.is_some_and(|k| (k - 0.5).abs() < 1e-12),
            "{read:?}"
        );
        assert_eq!(read.mean_difference, None);
    }

    /// Every pair one label: chance agrees on all, so no kappa is read.
    #[test]
    fn one_label_alone_reads_no_kappa() {
        let b = ScoreType::Boolean;
        let read = agreement(30, &[pair(b, 1.0, 1.0), pair(b, 1.0, 1.0)]);
        assert_eq!(read.share, Some(1.0));
        assert_eq!(read.kappa, None);
    }

    /// A category by its labels, sorted; a number by its mean difference.
    #[test]
    fn a_category_reads_its_labels_and_a_number_its_mean_difference() {
        let c = |judged: &str, given: &str| Pair {
            name: "tone".into(),
            data_type: ScoreType::Category,
            judged: 0.0,
            judged_label: judged.into(),
            given: 0.0,
            given_label: given.into(),
        };
        let read = agreement(7, &[c("polite", "polite"), c("rude", "polite")]);
        assert_eq!(read.labels, ["polite", "rude"]);
        assert_eq!(read.matrix, [[1, 0], [1, 0]]);
        assert_eq!(read.agreeing, Some(1));
        let n = ScoreType::Number;
        let read = agreement(
            7,
            &[pair(n, 0.8, 0.5), pair(n, 0.2, 0.5), pair(n, 1.0, 0.4)],
        );
        assert!(
            read.mean_difference
                .is_some_and(|d| (d - 0.2).abs() < 1e-12),
            "{read:?}"
        );
        assert_eq!((read.share, read.kappa), (None, None));
        assert_eq!(agreement(7, &[]).pairs, 0);
    }

    /// Each score name is read apart, by name, so two questions of one typed
    /// evaluator never meet in one matrix.
    #[test]
    fn each_score_name_is_read_apart() {
        let b = ScoreType::Boolean;
        let other = Pair {
            name: "followed".into(),
            ..pair(b, 0.0, 1.0)
        };
        let read = agreements(30, &[pair(b, 1.0, 1.0), other.clone(), other]);
        let names: Vec<&str> = read.scores.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["followed", "relevant"]);
        assert_eq!(
            (
                read.scores[0].pairs,
                read.scores[0].agreeing,
                read.scores[1].agreeing
            ),
            (2, Some(0), Some(1))
        );
        assert!(agreements(30, &[]).scores.is_empty());
    }
}
