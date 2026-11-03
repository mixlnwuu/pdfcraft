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
