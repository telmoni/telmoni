//! Splitting text into passages an embedding can hold, and the hash that
//! decides whether a passage needs embedding again.

use sha2::{Digest, Sha256};

use crate::db::Visibility;

/// About 800 tokens at four characters each: long enough to keep a section
/// whole, short enough that one vector still means one thing.
pub const MAX_CHARS: usize = 3_200;

/// One passage: the heading it sits under, when it has one, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
    pub heading: Option<String>,
    pub text: String,
}

/// Split Markdown at its `##` and `###` headings, then any section still
/// longer than [`MAX_CHARS`] at its blank lines, then any paragraph still
/// longer at a character boundary. Empty sections are dropped.
#[must_use]
pub fn split(markdown: &str) -> Vec<Passage> {
    let mut sections: Vec<Passage> = Vec::new();
    let mut heading: Option<String> = None;
    let mut body = String::new();
    let mut fence = Fence::default();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let in_code = fence.read(line);
        if !in_code
            && let Some(title) = trimmed
                .strip_prefix("### ")
                .or_else(|| trimmed.strip_prefix("## "))
        {
            flush(&mut sections, heading.take(), &mut body);
            heading = Some(title.trim().to_owned());
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    flush(&mut sections, heading, &mut body);
    sections
}

/// Whether a line of Markdown is code: inside a ``` or ~~~ fence, or the
/// fence line itself. A `# comment` in a shell block is not a heading.
#[derive(Debug, Default)]
pub struct Fence {
    open: Option<(char, usize)>,
}

impl Fence {
    /// Read the next line; answers whether it is code.
    pub fn read(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        let run = marker.map_or(0, |m| trimmed.chars().take_while(|c| *c == m).count());
        match (self.open, marker) {
            (None, Some(m)) if run >= 3 => {
                self.open = Some((m, run));
                true
            }
            (Some((m, len)), Some(c))
                if c == m && run >= len && trimmed.trim_start_matches(c).trim().is_empty() =>
            {
                self.open = None;
                true
            }
            (Some(_), _) => true,
            (None, _) => false,
        }
    }
}

fn flush(out: &mut Vec<Passage>, heading: Option<String>, body: &mut String) {
    let text = std::mem::take(body);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    for piece in cap(text) {
        out.push(Passage {
            heading: heading.clone(),
            text: piece,
        });
    }
}

/// Pieces of at most [`MAX_CHARS`], packed from whole paragraphs where they fit.
fn cap(text: &str) -> Vec<String> {
    if text.chars().count() <= MAX_CHARS {
        return vec![text.to_owned()];
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    for paragraph in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        let fits = current.chars().count() + paragraph.chars().count() + 2 <= MAX_CHARS;
        if !fits && !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
        }
        if paragraph.chars().count() > MAX_CHARS {
            let chars: Vec<char> = paragraph.chars().collect();
            pieces.extend(
                chars
                    .chunks(MAX_CHARS)
                    .map(|c| c.iter().collect::<String>()),
            );
            continue;
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// What a passage is, as far as its embedding and its reader care: a change
/// to any part is a passage to write again.
#[must_use]
pub fn content_hash(title: &str, body: &str, url: Option<&str>, visibility: Visibility) -> String {
    let mut hasher = Sha256::new();
    for part in [title, body, url.unwrap_or_default(), visibility.as_str()] {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn headings_start_passages_and_the_preamble_keeps_none() {
        let passages = split("Intro line.\n\n## Install\nRun it.\n### Flags\n--x\n## Empty\n\n");
        assert_eq!(
            passages,
            vec![
                Passage {
                    heading: None,
                    text: "Intro line.".into()
                },
                Passage {
                    heading: Some("Install".into()),
                    text: "Run it.".into()
                },
                Passage {
                    heading: Some("Flags".into()),
                    text: "--x".into()
                },
            ]
        );
    }

    #[test]
    fn a_heading_inside_a_code_fence_is_code() {
        let passages = split("## Install\n```bash\n## not a heading\n# nor this\n```\nDone.\n");
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].heading.as_deref(), Some("Install"));
        assert!(passages[0].text.contains("## not a heading"));
    }

    #[test]
    fn a_fence_closes_only_on_its_own_marker() {
        let mut fence = Fence::default();
        assert!(!fence.read("text"));
        assert!(fence.read("````md"));
        assert!(
            fence.read("```"),
            "a shorter run does not close a longer fence"
        );
        assert!(fence.read("~~~"), "another marker does not close it");
        assert!(fence.read("````"));
        assert!(!fence.read("## after"));
    }

    #[test]
    fn a_long_section_splits_at_paragraphs_and_a_long_paragraph_at_the_cap() {
        let paragraph = "a".repeat(2_000);
        let giant = "b".repeat(MAX_CHARS * 2 + 5);
        let text = format!("## S\n{paragraph}\n\n{paragraph}\n\n{giant}");
        let passages = split(&text);
        assert!(passages.iter().all(|p| p.text.chars().count() <= MAX_CHARS));
        assert!(passages.iter().all(|p| p.heading.as_deref() == Some("S")));
        assert_eq!(passages.len(), 5);
    }

    #[test]
    fn the_hash_moves_with_every_part_and_nothing_else() {
        let base = content_hash("t", "b", Some("/u"), Visibility::Everyone);
        assert_eq!(
            base,
            content_hash("t", "b", Some("/u"), Visibility::Everyone)
        );
        assert_ne!(
            base,
            content_hash("t", "b2", Some("/u"), Visibility::Everyone)
        );
        assert_ne!(base, content_hash("t", "b", None, Visibility::Everyone));
        // A passage whose readers changed is written again.
        let by_visibility: HashSet<String> = Visibility::all()
            .into_iter()
            .map(|visibility| content_hash("t", "b", Some("/u"), visibility))
            .collect();
        assert_eq!(by_visibility.len(), Visibility::all().len());
        // A boundary moved between parts is a different passage.
        assert_ne!(
            content_hash("ab", "c", None, Visibility::Author),
            content_hash("a", "bc", None, Visibility::Author)
        );
    }

    /// ⚠ Every row in the index carries its hash: one made another way from
    /// the same passage embeds the whole index again.
    #[test]
    fn the_hash_is_the_one_the_index_holds() {
        assert_eq!(
            content_hash("t", "b", Some("/u"), Visibility::Everyone),
            "821a3560ed976ec7ce99bb5f113a845a7dc19952d7cb2a8f0f630b3ecbc41576"
        );
    }
}
