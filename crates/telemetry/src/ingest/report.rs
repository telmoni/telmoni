//! OTLP's partial success: unset when nothing was refused or dropped, else the
//! spans refused counted, and each refusal and drop named with why, to 4 KiB.

use opentelemetry_proto::tonic::collector::trace::v1::ExportTracePartialSuccess;

use telmoni_shared::ContentMode;

use crate::contract::ERROR_MESSAGE_MAX;
use crate::ingest::span::{DropReason, Dropped, Refusal, Refused};

/// What one export lost on the way in.
#[derive(Debug, Default)]
pub struct Report {
    /// The spans refused whole, in the order they came.
    pub refused: Vec<Refused>,
    /// The attributes dropped from spans that were kept, in the order they
    /// came.
    pub dropped: Drops,
}

/// How many drops are kept by name: each line names its span's sixteen hex
/// digits, so no message names more.
pub const DROPS_NAMED_MAX: usize = ERROR_MESSAGE_MAX / 16;

/// The attributes dropped: the first [`DROPS_NAMED_MAX`] by name, the rest counted,
/// so an export of empty attributes holds no more than its answer can name.
#[derive(Debug, Default)]
pub struct Drops {
    named: Vec<Dropped>,
    counted: usize,
}

impl Drops {
    /// One more dropped: by name while there is room, else counted.
    pub fn push(&mut self, span_id: &str, attribute: &str, reason: DropReason) {
        if self.named.len() < DROPS_NAMED_MAX {
            self.named.push(Dropped {
                span_id: span_id.to_owned(),
                attribute: attribute.to_owned(),
                reason,
            });
        } else {
            self.counted += 1;
        }
    }

    /// How many were dropped, named or counted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.named.len() + self.counted
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Back to the first `len` dropped, as a `Vec` is truncated: the named
    /// fill first, so the count holds only what came past them.
    pub fn truncate(&mut self, len: usize) {
        self.named.truncate(len);
        self.counted = self.counted.min(len.saturating_sub(self.named.len()));
    }

    /// The drops kept by name, in the order they came.
    #[must_use]
    pub fn named(&self) -> &[Dropped] {
        &self.named
    }
}

impl Report {
    /// Whether no span was refused and no attribute dropped.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.refused.is_empty() && self.dropped.is_empty()
    }

    /// OTLP's `partial_success`: `None` when nothing was refused or dropped.
    #[must_use]
    pub fn partial_success(&self, content_mode: ContentMode) -> Option<ExportTracePartialSuccess> {
        if self.is_clean() {
            return None;
        }
        Some(ExportTracePartialSuccess {
            rejected_spans: i64::try_from(self.refused.len()).unwrap_or(i64::MAX),
            error_message: self.message(content_mode),
        })
    }

    /// The lines, refusals first, cut at [`ERROR_MESSAGE_MAX`] with a count
    /// of what was left out.
    fn message(&self, content_mode: ContentMode) -> String {
        let lines = self.refused.iter().map(refused_line).chain(
            self.dropped
                .named()
                .iter()
                .map(|d| dropped_line(d, content_mode)),
        );
        cut(lines, self.refused.len() + self.dropped.len())
    }
}

/// `lines` joined while they fit in [`ERROR_MESSAGE_MAX`], then "and N more"
/// for the rest of `total`, counted and never formatted: an export of a
/// hundred thousand drops formats only the lines its answer can hold.
fn cut(lines: impl Iterator<Item = String>, total: usize) -> String {
    let mut message = String::new();
    let mut named = 0usize;
    for line in lines {
        let needed = message.len() + line.len() + usize::from(!message.is_empty());
        if needed > ERROR_MESSAGE_MAX {
            break;
        }
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str(&line);
        named += 1;
    }
    let left_out = total.saturating_sub(named);
    if left_out > 0 {
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str(&format!("and {left_out} more"));
    }
    message
}

fn refused_line(refused: &Refused) -> String {
    let why = match &refused.reason {
        Refusal::NotARun => {
            "no telmoni.run_id and telmoni.agent; a span outside a Telmoni run is refused"
                .to_owned()
        }
        Refusal::Invalid(field) => format!("invalid {field}"),
        Refusal::OverCap(field) => format!("{field} over its cap"),
    };
    format!("span {} refused: {why}", refused.span_id)
}

fn dropped_line(dropped: &Dropped, content_mode: ContentMode) -> String {
    let why = match &dropped.reason {
        DropReason::Content => {
            format!("content; the project's content mode is {content_mode}")
        }
        DropReason::Unlisted => {
            "unlisted; text the field inventory does not name is never kept".to_owned()
        }
        DropReason::OverCap(cap) => format!("over_cap; {cap}"),
    };
    format!(
        "span {} dropped {}: {why}",
        dropped.span_id, dropped.attribute
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Content dropped as `gen_ai.input.messages.<n>`, for each `n`.
    fn dropped(numbers: std::ops::Range<usize>) -> Drops {
        let mut drops = Drops::default();
        for n in numbers {
            drops.push(
                "00f067aa0ba902b7",
                &format!("gen_ai.input.messages.{n}"),
                DropReason::Content,
            );
        }
        drops
    }

    #[test]
    fn a_clean_export_gets_no_partial_success() {
        assert!(
            Report::default()
                .partial_success(ContentMode::Off)
                .is_none()
        );
    }

    #[test]
    fn refusals_are_counted_and_named_before_drops() {
        let report = Report {
            refused: vec![Refused {
                span_id: "0f0f0f0f0f0f0f0f".into(),
                reason: Refusal::NotARun,
            }],
            dropped: dropped(1..2),
        };
        let partial = report.partial_success(ContentMode::Off).expect("not clean");
        assert_eq!(partial.rejected_spans, 1);
        let lines: Vec<&str> = partial.error_message.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("span 0f0f0f0f0f0f0f0f refused: no telmoni.run_id"));
        assert_eq!(
            lines[1],
            "span 00f067aa0ba902b7 dropped gen_ai.input.messages.1: content; the project's \
             content mode is off"
        );
    }

    /// ⚠ The message stops at 4 KiB: a batch of a thousand spans each
    /// dropping content would otherwise answer with more than it was sent.
    #[test]
    fn the_message_is_cut_at_four_kib_with_a_count_of_the_rest() {
        let report = Report {
            refused: Vec::new(),
            dropped: dropped(0..200),
        };
        let partial = report.partial_success(ContentMode::Off).expect("not clean");
        assert_eq!(partial.rejected_spans, 0);
        let message = partial.error_message;
        assert!(
            message.len() <= ERROR_MESSAGE_MAX + "\nand 999 more".len(),
            "{}",
            message.len()
        );
        let last = message.lines().last().expect("a last line");
        assert!(
            last.starts_with("and ") && last.ends_with(" more"),
            "{last}"
        );
        let named = message.lines().count() - 1;
        let counted: usize = last
            .trim_start_matches("and ")
            .trim_end_matches(" more")
            .parse()
            .expect("a count");
        assert_eq!(named + counted, 200, "every drop is named or counted");
    }

    /// Past the first line that does not fit, no line is formatted.
    #[test]
    fn lines_past_the_cap_are_counted_never_formatted() {
        let formatted = std::cell::Cell::new(0usize);
        let lines = (0..100_000).map(|n| {
            formatted.set(formatted.get() + 1);
            format!("span 00f067aa0ba902b7 dropped gen_ai.input.messages.{n}: content")
        });
        let message = cut(lines, 100_000);
        let named = message.lines().count() - 1;
        assert_eq!(
            formatted.get(),
            named + 1,
            "the line that did not fit, and none after"
        );
        assert!(message.ends_with(&format!("\nand {} more", 100_000 - named)));
    }

    /// ⚠ A drop past what a message can name is counted, never kept, however many come.
    #[test]
    fn drops_past_what_a_message_can_name_are_counted_never_kept() {
        let report = Report {
            refused: Vec::new(),
            dropped: dropped(0..100_000),
        };
        assert_eq!(
            (report.dropped.named().len(), report.dropped.len()),
            (DROPS_NAMED_MAX, 100_000)
        );
        let message = report
            .partial_success(ContentMode::Off)
            .expect("not clean")
            .error_message;
        let named = message.lines().count() - 1;
        assert!(message.ends_with(&format!("\nand {} more", 100_000 - named)));
    }

    /// The shortest line names its span, so a message runs out before the names kept do.
    #[test]
    fn a_message_of_the_shortest_lines_names_fewer_than_are_kept() {
        let mut drops = Drops::default();
        for _ in 0..=DROPS_NAMED_MAX {
            drops.push(
                "00f067aa0ba902b7",
                "",
                DropReason::OverCap("a span keeps up to 64 attributes"),
            );
        }
        let message = Report {
            refused: Vec::new(),
            dropped: drops,
        }
        .partial_success(ContentMode::Off)
        .expect("not clean")
        .error_message;
        assert!(message.lines().count() - 1 < DROPS_NAMED_MAX, "{message}");
    }

    #[test]
    fn drops_are_truncated_as_a_vec_is() {
        let mut drops = dropped(0..DROPS_NAMED_MAX + 10);
        drops.truncate(DROPS_NAMED_MAX + 3);
        assert_eq!(
            (drops.named().len(), drops.len()),
            (DROPS_NAMED_MAX, DROPS_NAMED_MAX + 3)
        );
        drops.truncate(5);
        assert_eq!((drops.named().len(), drops.len()), (5, 5));
        drops.push("00f067aa0ba902b7", "after", DropReason::Unlisted);
        assert_eq!(
            drops.named().last().map(|d| d.attribute.as_str()),
            Some("after")
        );
    }
}
