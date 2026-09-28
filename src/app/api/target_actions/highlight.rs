//! Request-only mapping of the winning source spelling to viewport cells.
//! Canonical file paths must not be searched in the displayed text.
use crate::file_url_actions::TargetCellSpan;

fn push_cell(spans: &mut Vec<TargetCellSpan>, row: u16, start: u16, end: u16) {
    if start >= end {
        return;
    }
    if let Some(last) = spans.last_mut().filter(|last| last.row == row) {
        last.start_col = last.start_col.min(start);
        last.end_col = last.end_col.max(end);
    } else {
        spans.push(TargetCellSpan {
            row,
            start_col: start,
            end_col: end,
        });
    }
}

pub(super) fn source_spans(
    text: &str,
    clicked: usize,
    literal: &str,
    width: u16,
    height: u16,
) -> Vec<TargetCellSpan> {
    if literal.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }
    // Both the URL parser and winning file candidate preserve their original
    // source spelling. The pointer disambiguates repeated occurrences.
    let Some((start, _)) = text
        .match_indices(literal)
        .find(|(start, value)| clicked >= *start && clicked < *start + value.len())
    else {
        return Vec::new();
    };
    let end = start + literal.len();
    let mut spans = Vec::new();
    for cell in crate::app::actions::visible_text_cells(text, width) {
        if cell.screen_row >= height {
            break;
        }
        if (start..end).contains(&cell.byte_index) {
            let cells = u16::from(crate::ghostty::unicode_codepoint_width(cell.ch as u32));
            // Zero-width marks are part of the preceding painted grapheme.
            if cells > 0 {
                push_cell(
                    &mut spans,
                    cell.screen_row,
                    cell.screen_col,
                    cell.screen_col.saturating_add(cells).min(width),
                );
            }
        }
    }
    spans
}

pub(super) fn hyperlink_spans(
    links: &[((u16, u16), String, String)],
    clicked: usize,
    width: u16,
    height: u16,
) -> Vec<TargetCellSpan> {
    let Some((_, _, uri)) = links.get(clicked) else {
        return Vec::new();
    };
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let linear = |index: usize| {
        let ((x, y), _, _) = &links[index];
        u32::from(*y) * u32::from(width) + u32::from(*x)
    };
    // Preserve only the contiguous OSC-8 label containing the click; another
    // occurrence of the same URI elsewhere must not be highlighted.
    let adjacent =
        |a: usize, b: usize| links[a].2 == *uri && links[b].2 == *uri && linear(a) + 1 == linear(b);
    let mut first = clicked;
    let mut last = clicked;
    while first > 0 && adjacent(first - 1, first) {
        first -= 1;
    }
    while last + 1 < links.len() && adjacent(last, last + 1) {
        last += 1;
    }
    let mut spans = Vec::new();
    for ((x, y), _, _) in &links[first..=last] {
        if *x < width && *y < height {
            push_cell(&mut spans, *y, *x, x.saturating_add(1).min(width));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    fn span(row: u16, start_col: u16, end_col: u16) -> TargetCellSpan {
        TargetCellSpan {
            row,
            start_col,
            end_col,
        }
    }

    #[test]
    fn source_span_uses_clicked_occurrence_and_original_not_canonical_path() {
        let text = "a.rs a.rs";
        assert_eq!(source_spans(text, 6, "a.rs", 30, 2), vec![span(0, 5, 9)]);
        assert!(source_spans(text, 6, "/tmp/a.rs", 30, 2).is_empty());
        assert!(source_spans(text, 4, "a.rs", 30, 2).is_empty());
    }

    #[test]
    fn source_span_tracks_wrapping_quotes_suffixes_and_terminal_cell_widths() {
        let text = "x\n\"a 文e\u{301}.txt\":4:2";
        let literal = text.split_once('\n').unwrap().1;
        assert_eq!(
            source_spans(text, 5, literal, 10, 4),
            vec![span(1, 0, 10), span(2, 0, 5)]
        );
        assert_eq!(source_spans(text, 5, literal, 10, 2), vec![span(1, 0, 10)]);
    }

    #[test]
    fn url_span_excludes_surrounding_prose_and_covers_wrapped_continuation() {
        let url = "https://a.test/p?q=1#s";
        let text = format!("({url}).");
        assert_eq!(
            source_spans(&text, 18, url, 12, 3),
            vec![span(0, 1, 12), span(1, 0, 11)]
        );
    }

    #[test]
    fn hyperlink_span_stops_at_gaps_and_different_destinations() {
        let link = |x, y, uri: &str| ((x, y), "x".into(), uri.into());
        let links = vec![
            link(0, 0, "u"),
            link(3, 0, "u"),
            link(4, 0, "u"),
            link(0, 1, "u"),
            link(1, 1, "other"),
            link(2, 1, "u"),
        ];
        assert_eq!(
            hyperlink_spans(&links, 2, 5, 2),
            vec![span(0, 3, 5), span(1, 0, 1)]
        );
        assert_eq!(hyperlink_spans(&links, 0, 5, 2), vec![span(0, 0, 1)]);
        assert!(hyperlink_spans(&links, 99, 5, 2).is_empty());
    }
}
