//! The underlines a rewrite draws under existing text (Thai edition). They are paths, not part of
//! the text, so each is tagged `/PCUnderline BMC … EMC` right after the text it underlines; the
//! next rewrite of that text (a new style, new words, deleting it, moving it) finds them there,
//! removes them with the text and draws them again only while the text stays underlined.

use std::collections::HashSet;
use std::ops::RangeInclusive;

use super::{Object, Op, TextLine};

/// The marked-content tag around an underline.
const TAG: &[u8] = b"PCUnderline";

/// `ops` (an underline's drawing) inside its tag.
pub(crate) fn tagged(ops: Vec<Op>) -> Vec<Op> {
    let mut out = Vec::with_capacity(ops.len().saturating_add(2));
    out.push(Op::new("BMC", vec![Object::name("PCUnderline")]));
    out.extend(ops);
    out.push(Op::new("EMC", vec![]));
    out
}

/// The tagged underlines drawn right after the runs of `members`: each one's operators, its `BMC`
/// to its `EMC`. Only text-closing operators (`ET`, `EMC`, `Ts`, and a `TJ` that only moves,
/// as a group move leaves) may come between a run and its underline, which is how a rewrite
/// writes them.
pub(crate) fn own_underlines(ops: &[Op], members: &[&TextLine]) -> Vec<RangeInclusive<usize>> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for &run in members.iter().flat_map(|l| l.ops.iter()) {
        let Some(start) = run.checked_add(1) else { continue };
        let Some(open) = (start..ops.len()).find(|&i| ops.get(i).is_none_or(|op| !closes_text(op))) else { continue };
        if !ops.get(open).is_some_and(is_tag) {
            continue;
        }
        let Some(close) = (open..ops.len()).find(|&i| ops.get(i).is_some_and(|op| op.is("EMC"))) else { continue };
        if seen.insert(open) {
            found.push(open..=close);
        }
    }
    found
}

/// Whether a tagged underline follows run operator `run` (see [`own_underlines`]).
fn underline_after(ops: &[Op], run: usize) -> bool {
    run.checked_add(1)
        .and_then(|start| (start..ops.len()).find(|&i| ops.get(i).is_none_or(|op| !closes_text(op))))
        .is_some_and(|open| ops.get(open).is_some_and(is_tag))
}

/// Thai edition: the parts of `line`'s text (byte ranges) that a tagged underline follows. A part
/// underlined on its own, joined into one line with text that isn't, is told apart by its run;
/// a line whose runs can't be told apart is whole when any of it is underlined.
pub(crate) fn underlined_parts(ops: &[Op], line: &TextLine) -> Vec<std::ops::Range<usize>> {
    let runs: Vec<(&String, &super::Placed)> = line.runs.iter().map(|r| &r.0).zip(line.placed.iter()).collect();
    if runs.is_empty() || runs.len() != line.placed.len() || runs.len() != line.runs.len() {
        return if own_underlines(ops, &[line]).is_empty() { Vec::new() } else { std::iter::once(0..line.text.len()).collect() };
    }
    let mut parts: Vec<std::ops::Range<usize>> = Vec::new();
    let mut from = 0;
    for (text, placed) in runs {
        let piece = text.trim();
        let Some(at) = line.text.get(from..).and_then(|rest| rest.find(piece)).map(|k| k + from) else { continue };
        let end = at + piece.len();
        from = end;
        if piece.is_empty() || !placed.ops.iter().any(|&op| underline_after(ops, op)) {
            continue;
        }
        match parts.last_mut() {
            // Runs side by side, or with only spaces between, are one part.
            Some(last) if line.text.get(last.end..at).is_some_and(|gap| gap.trim().is_empty()) => last.end = end,
            _ => parts.push(at..end),
        }
    }
    parts
}

/// Every operator index in `ranges`.
pub(crate) fn indexes(ranges: &[RangeInclusive<usize>]) -> impl Iterator<Item = usize> + '_ {
    ranges.iter().flat_map(Clone::clone)
}

fn closes_text(op: &Op) -> bool {
    match op.op.as_slice() {
        b"ET" | b"EMC" | b"Ts" => true,
        b"TJ" => matches!(op.operands.as_slice(), [Object::Array(a)] if a.iter().all(|v| v.as_f64().is_some())),
        _ => false,
    }
}

fn is_tag(op: &Op) -> bool {
    op.is("BMC") && matches!(op.operands.first(), Some(Object::Name(n)) if n.as_slice() == TAG)
}

#[cfg(test)]
mod tests {
    use super::super::{BlockStyle, RangeStyle, lines, rewrite_block, text_blocks, text_lines};

    /// The words underlined on page 0, part by part.
    fn underlined(doc: &pdfcraft_cos::Document) -> Vec<String> {
        let page = lines::page_dict(doc, 0).unwrap();
        let streams = lines::content_streams(doc, &page.dict).unwrap();
        let ops = super::super::joined(&streams).parse().ops;
        text_lines(doc, 0)
            .unwrap()
            .iter()
            .flat_map(|l| super::underlined_parts(&ops, l).into_iter().filter_map(|r| l.text.get(r).map(str::to_owned)).collect::<Vec<_>>())
            .collect()
    }

    /// Thai edition: a part underlined on its own keeps its underline, alone, through the next
    /// rewrite of its paragraph (it was read as the whole paragraph's and underlined all of it).
    #[test]
    fn a_part_underlined_alone_stays_alone_through_a_rewrite() {
        let mut doc = crate::tests::text_page("BT /F1 12 Tf 72 700 Td (Hello brave world) Tj ET");
        let range =
            RangeStyle { range: 0..5, family: None, resource: None, size: None, color: None, underline: Some(true), char_spacing: None, scale: None };
        rewrite_block(&mut doc, 0, 0, None, &BlockStyle { ranges: vec![range], ..Default::default() }).unwrap();
        assert_eq!(underlined(&doc), vec!["Hello".to_string()]);
        let b = text_blocks(&doc, 0).unwrap().remove(0);
        rewrite_block(&mut doc, 0, 0, Some(&b.editor_text), &BlockStyle::default()).unwrap();
        assert_eq!(underlined(&doc), vec!["Hello".to_string()], "still only Hello");
    }
    /// Thai edition: underlining a part keeps the paragraph one paragraph. The part is drawn in a
    /// text object of its own, touching the rest ("เรื่อง" against its dotted leader), inside the
    /// rewrite's marked-content scope; read as a separate label touching it, it split the
    /// paragraph, and the open editor lost its selection.
    #[test]
    fn touching_text_objects_in_one_marked_scope_are_one_paragraph() {
        // Helvetica "Hello" at 12 pt is 27.336 pt wide: the dots, in a content stream of their
        // own, start where it ends.
        let (hello, dots) = ("BT /F1 12 Tf 72 700 Td (Hello) Tj ET", "BT /F1 12 Tf 99.336 700 Td (....) Tj ET");
        let doc = crate::tests::split_page(&format!("/P BMC {hello}"), &format!("{dots} EMC"));
        let blocks = text_blocks(&doc, 0).unwrap();
        assert_eq!(blocks.iter().map(|b| b.editor_text.as_str()).collect::<Vec<_>>(), vec!["Hello...."], "{blocks:?}");
        // Without a scope of their own they stay apart, as labels dragged against each other do (#594).
        assert_eq!(text_blocks(&crate::tests::split_page(hello, dots), 0).unwrap().len(), 2);
    }
}
