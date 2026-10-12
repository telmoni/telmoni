//! CSV, as the downloads write it: the audit log's export and the trace list's.

/// What a file opens with: the byte-order mark is how a spreadsheet knows the
/// file is UTF-8. Without it, Excel reads a name with an accent as two wrong
/// characters.
pub const BOM: &str = "\u{feff}";

/// One CSV cell, as RFC 4180 has it: quoted when it holds a comma, a quote or
/// a line break, its quotes doubled. And defused, as OWASP advises: a cell a
/// spreadsheet would run as a formula — one starting with `=`, `+`, `-`, `@`,
/// a tab or a carriage return — starts with `'` inside its quotes instead,
/// since what the cells carry was typed by people — an event's details, an
/// agent's name, a session's id — and the file opens in somebody's
/// spreadsheet.
pub fn cell(out: &mut Vec<u8>, cell: &str) {
    let defuse = cell.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let quote = defuse || cell.contains([',', '"', '\n', '\r']);
    if quote {
        out.push(b'"');
    }
    if defuse {
        out.push(b'\'');
    }
    out.extend_from_slice(cell.replace('"', "\"\"").as_bytes());
    if quote {
        out.push(b'"');
    }
}

/// One CSV row: its cells, then the line break RFC 4180 names.
pub fn row<S: AsRef<str>>(out: &mut Vec<u8>, cells: &[S]) {
    for (i, value) in cells.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        cell(out, value.as_ref());
    }
    out.extend_from_slice(b"\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(value: &str) -> String {
        let mut out = Vec::new();
        cell(&mut out, value);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_plain_cell_is_written_as_it_is() {
        assert_eq!(written("created"), "created");
        assert_eq!(written(""), "");
    }

    #[test]
    fn a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted() {
        assert_eq!(written("a,b"), "\"a,b\"");
        assert_eq!(written("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(written("two\nlines"), "\"two\nlines\"");
        assert_eq!(
            written("{\"name\":\"Ada\",\"role\":\"admin\"}"),
            "\"{\"\"name\"\":\"\"Ada\"\",\"\"role\"\":\"\"admin\"\"}\""
        );
    }

    #[test]
    fn a_cell_a_spreadsheet_would_run_is_defused() {
        assert_eq!(written("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(written("+1"), "\"'+1\"");
        assert_eq!(written("-1"), "\"'-1\"");
        assert_eq!(written("@SUM(A1)"), "\"'@SUM(A1)\"");
        assert_eq!(written("\tx"), "\"'\tx\"");
    }

    #[test]
    fn a_row_is_its_cells_by_commas_then_a_crlf() {
        let mut out = Vec::new();
        row(&mut out, &["a", "b,c", "=1"]);
        assert_eq!(String::from_utf8(out).unwrap(), "a,\"b,c\",\"'=1\"\r\n");
    }
}
