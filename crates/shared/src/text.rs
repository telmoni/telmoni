//! Bounded and encoded text.

use crate::digest::hex_digit;

/// `s` as a query-string value: percent-encoded, a space `%20` and never `+`.
#[must_use]
pub fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            other => {
                out.push('%');
                out.push(hex_digit(other >> 4).to_ascii_uppercase());
                out.push(hex_digit(other).to_ascii_uppercase());
            }
        }
    }
    out
}

/// `s` cut to at most `max` **bytes**, never inside a character.
#[must_use]
pub fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    (0..=max)
        .rev()
        .find_map(|end| Some(s.split_at_checked(end)?.0))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::{truncate_on_char_boundary, urlencoding};

    #[test]
    fn a_query_value_is_percent_encoded_with_no_plus() {
        assert_eq!(urlencoding("a b&c=d/~x.y_z-0"), "a%20b%26c%3Dd%2F~x.y_z-0");
        assert_eq!(urlencoding("é"), "%C3%A9");
        assert_eq!(urlencoding(""), "");
    }

    #[test]
    fn a_short_string_is_returned_whole() {
        assert_eq!(truncate_on_char_boundary("abc", 10), "abc");
        assert_eq!(truncate_on_char_boundary("abc", 3), "abc");
    }

    #[test]
    fn the_cut_never_lands_inside_a_character() {
        let s = "A\u{1F680}\u{1F680}";
        let cut = truncate_on_char_boundary(s, 8);
        assert_eq!(cut, "A\u{1F680}");
        assert!(cut.len() <= 8);
    }

    #[test]
    fn a_limit_below_the_first_character_gives_an_empty_string() {
        assert_eq!(truncate_on_char_boundary("\u{1F680}", 2), "");
    }

    #[test]
    fn the_result_never_exceeds_the_byte_limit() {
        for max in 0..12 {
            assert!(truncate_on_char_boundary("a\u{e9}\u{4e2d}\u{1F680}b", max).len() <= max);
        }
    }
}
