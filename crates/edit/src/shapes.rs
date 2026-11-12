//! A page's own drawn rectangles and straight rules (Thai edition), for exports that rebuild a
//! table's grid, its cell shading, a shaded box or a signature line from what the page draws
//! rather than guessing from where the text sits.
//!
//! Only the page's content stream is read, as [`crate::page_background`] does: Form XObjects,
//! shadings, patterns, curves and clipping are left out. A rule is a straight stroke along x or
//! y, or a filled rectangle thin enough to be one (a producer's way of drawing a table border).

use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::Document;

use crate::{EditError, Rgb};

/// How deep `q` may nest; deeper saves are not tracked.
const MAX_SAVES: usize = 64;

/// The most shapes one page gives; a page that draws more (a hatched chart) gives the first.
pub const MAX_SHAPES: usize = 20_000;

/// A filled rectangle no thicker than this (points) is a rule, not an area.
const RULE_THICKNESS: f64 = 2.5;

/// How far (points) a stroke's ends may differ across its length and still run along x or y.
const STRAIGHT: f64 = 0.5;

/// A rectangle the page fills, or a rule it draws, in user space (y up).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnShape {
    /// [x0, y0, x1, y1], x0 ≤ x1 and y0 ≤ y1. A rule's thin side is its thickness.
    pub rect: [f64; 4],
    pub color: Rgb,
    /// A line (stroked, or a fill no thicker than [`RULE_THICKNESS`]); otherwise a filled area.
    pub rule: bool,
    /// A small filled curved shape, no bigger than [`DOT`] either way: a list's bullet, as a
    /// browser or a word processor draws a disc or a circle. Its `rect` is its box.
    pub dot: bool,
}

/// A filled curved shape no wider or taller than this (points) is a dot.
const DOT: f64 = 8.0;

/// The most points of a curved path kept for its box.
const MAX_CURVE_POINTS: usize = 64;

#[derive(Clone)]
struct State {
    ctm: Matrix,
    fill: Option<Rgb>,
    stroke: Option<Rgb>,
    /// Fill and stroke colour spaces: 1 gray, 3 RGB, 4 CMYK, 0 anything else.
    fill_space: u8,
    stroke_space: u8,
    width: f64,
}

/// The page's filled rectangles and straight rules, in drawing order (see the module).
pub fn reading_shapes(doc: &Document, page: usize) -> Result<Vec<DrawnShape>, EditError> {
    let pages = pdfcraft_model::pages(doc);
    let p = pages.get(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = crate::text::content_streams(doc, &p.dict)?;
    let pieces = crate::text::joined(&streams);
    Ok(shapes(&pieces.parse().ops))
}

/// [`reading_shapes`] over a parsed content stream.
pub(crate) fn shapes(ops: &[Op]) -> Vec<DrawnShape> {
    let mut st = State { ctm: Matrix::IDENTITY, fill: Some([0.0; 3]), stroke: Some([0.0; 3]), fill_space: 1, stroke_space: 1, width: 1.0 };
    let mut saved: Vec<State> = Vec::new();
    let mut lost_saves = 0usize;
    // The path: its rectangles and its straight segments, in page space.
    let mut rects: Vec<[(f64, f64); 4]> = Vec::new();
    let mut segments: Vec<((f64, f64), (f64, f64))> = Vec::new();
    let mut start: Option<(f64, f64)> = None;
    let mut at: Option<(f64, f64)> = None;
    // A path with curves: the points it passes (for a dot's box), and whether it has any.
    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut curved = false;
    let mut out = Vec::new();
    for op in ops {
        if out.len() >= MAX_SHAPES {
            break;
        }
        match op.op.as_slice() {
            b"q" => {
                if saved.len() < MAX_SAVES {
                    saved.push(st.clone());
                } else {
                    lost_saves = lost_saves.saturating_add(1);
                }
            }
            b"Q" => {
                if lost_saves > 0 {
                    lost_saves -= 1;
                } else if let Some(s) = saved.pop() {
                    st = s;
                }
            }
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    st.ctm = m.then(&st.ctm);
                }
            }
            b"w" => {
                if let Some([w]) = op.nums::<1>() {
                    st.width = if w.is_finite() { w.abs() } else { 1.0 };
                }
            }
            b"g" => (st.fill_space, st.fill) = (1, op.nums::<1>().map(|[g]| [g; 3])),
            b"rg" => (st.fill_space, st.fill) = (3, op.nums::<3>()),
            b"k" => (st.fill_space, st.fill) = (4, op.nums::<4>().map(cmyk)),
            b"G" => (st.stroke_space, st.stroke) = (1, op.nums::<1>().map(|[g]| [g; 3])),
            b"RG" => (st.stroke_space, st.stroke) = (3, op.nums::<3>()),
            b"K" => (st.stroke_space, st.stroke) = (4, op.nums::<4>().map(cmyk)),
            b"cs" => (st.fill_space, st.fill) = (space(op), None),
            b"CS" => (st.stroke_space, st.stroke) = (space(op), None),
            b"sc" | b"scn" => st.fill = colour(st.fill_space, op),
            b"SC" | b"SCN" => st.stroke = colour(st.stroke_space, op),
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    let c = &st.ctm;
                    rects.push([c.apply(x, y), c.apply(x + w, y), c.apply(x + w, y + h), c.apply(x, y + h)]);
                }
            }
            b"m" => {
                if let Some([x, y]) = op.nums::<2>() {
                    let p = st.ctm.apply(x, y);
                    (start, at) = (Some(p), Some(p));
                    keep(&mut points, &[p]);
                }
            }
            b"l" => {
                if let (Some([x, y]), Some(from)) = (op.nums::<2>(), at) {
                    let p = st.ctm.apply(x, y);
                    segments.push((from, p));
                    at = Some(p);
                    keep(&mut points, &[p]);
                }
            }
            b"h" => {
                if let (Some(s), Some(from)) = (start, at) {
                    segments.push((from, s));
                    at = Some(s);
                }
            }
            // Curves end the straight path here: what they draw isn't a rule or a box, but a small
            // filled one is a dot.
            b"c" | b"v" | b"y" => {
                curved = true;
                at = None;
                let n = if op.is("c") { 3 } else { 2 };
                let nums: Vec<f64> = op.operands.iter().filter_map(pdfcraft_cos::Object::as_f64).collect();
                if nums.len() == 2 * n {
                    let ps: Vec<(f64, f64)> = nums.chunks(2).filter_map(|c| Some(st.ctm.apply(*c.first()?, *c.get(1)?))).collect();
                    keep(&mut points, &ps);
                }
            }
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"n" => {
                let paint = op.op.as_slice();
                let fills = matches!(paint, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
                let strokes = matches!(paint, b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s");
                if fills && let Some(color) = st.fill {
                    for q in &rects {
                        if let Some(r) = upright(q) {
                            let thin = (r[2] - r[0]).min(r[3] - r[1]) <= RULE_THICKNESS;
                            out.push(DrawnShape { rect: r, color: clamp(color), rule: thin, dot: false });
                        }
                    }
                    if curved
                        && let Some(r) = bounds(&points).filter(|r| (r[2] - r[0]).max(r[3] - r[1]) <= DOT && (r[2] - r[0]).min(r[3] - r[1]) >= 0.5)
                    {
                        out.push(DrawnShape { rect: r, color: clamp(color), rule: false, dot: true });
                    }
                }
                if strokes && let Some(color) = st.stroke {
                    let [a, b, c, d, _, _] = st.ctm.0;
                    let scale = (a * d - b * c).abs().sqrt();
                    let thick = (st.width * if scale.is_finite() { scale } else { 1.0 }).max(0.25);
                    let edges = rects.iter().flat_map(|q| [(q[0], q[1]), (q[1], q[2]), (q[2], q[3]), (q[3], q[0])]);
                    for (p0, p1) in segments.iter().copied().chain(edges) {
                        if let Some(r) = rule(p0, p1, thick) {
                            out.push(DrawnShape { rect: r, color: clamp(color), rule: true, dot: false });
                        }
                    }
                }
                rects.clear();
                segments.clear();
                points.clear();
                curved = false;
                (start, at) = (None, None);
            }
            _ => {}
        }
    }
    out.truncate(MAX_SHAPES);
    out
}

/// Adds `ps` to a curved path's points, up to [`MAX_CURVE_POINTS`].
fn keep(points: &mut Vec<(f64, f64)>, ps: &[(f64, f64)]) {
    for p in ps {
        if points.len() < MAX_CURVE_POINTS && p.0.is_finite() && p.1.is_finite() {
            points.push(*p);
        }
    }
}

/// The box around `points`; `None` for none.
fn bounds(points: &[(f64, f64)]) -> Option<[f64; 4]> {
    let first = points.first()?;
    Some(points.iter().fold([first.0, first.1, first.0, first.1], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]))
}

/// A rectangle whose corners lie along x and y: its box. `None` under a rotation or shear that
/// leaves it slanted, or for coordinates that aren't finite.
fn upright(q: &[(f64, f64); 4]) -> Option<[f64; 4]> {
    let xs = q.map(|p| p.0);
    let ys = q.map(|p| p.1);
    if !xs.iter().chain(&ys).all(|v| v.is_finite()) {
        return None;
    }
    let straight = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() <= STRAIGHT || (a.1 - b.1).abs() <= STRAIGHT;
    if !(straight(q[0], q[1]) && straight(q[1], q[2])) {
        return None;
    }
    let [x0, x1] = [xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max)];
    let [y0, y1] = [ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max)];
    Some([x0, y0, x1, y1])
}

/// A stroke from `a` to `b` that runs along x or y, as the thin rectangle it covers.
fn rule(a: (f64, f64), b: (f64, f64), thick: f64) -> Option<[f64; 4]> {
    if ![a.0, a.1, b.0, b.1, thick].iter().all(|v| v.is_finite()) {
        return None;
    }
    let half = thick / 2.0;
    if (a.1 - b.1).abs() <= STRAIGHT && (a.0 - b.0).abs() > STRAIGHT {
        let y = (a.1 + b.1) / 2.0;
        Some([a.0.min(b.0), y - half, a.0.max(b.0), y + half])
    } else if (a.0 - b.0).abs() <= STRAIGHT && (a.1 - b.1).abs() > STRAIGHT {
        let x = (a.0 + b.0) / 2.0;
        Some([x - half, a.1.min(b.1), x + half, a.1.max(b.1)])
    } else {
        None
    }
}

fn space(op: &Op) -> u8 {
    match op.name(0) {
        Some(b"DeviceGray" | b"G") => 1,
        Some(b"DeviceRGB" | b"RGB") => 3,
        Some(b"DeviceCMYK" | b"CMYK") => 4,
        _ => 0,
    }
}

/// The colour `sc`/`scn` (or `SC`/`SCN`) sets in a space of `n` components, if a plain one.
fn colour(n: u8, op: &Op) -> Option<Rgb> {
    match (n, op.operands.len()) {
        (1, 1) => op.nums::<1>().map(|[g]| [g; 3]),
        (3, 3) => op.nums::<3>(),
        (4, 4) => op.nums::<4>().map(cmyk),
        _ => None,
    }
}

fn cmyk([c, m, y, k]: [f64; 4]) -> Rgb {
    [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
}

fn clamp(c: Rgb) -> Rgb {
    c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(content: &str) -> Vec<DrawnShape> {
        shapes(&pdfcraft_content::parse(content.as_bytes()).ops)
    }

    #[test]
    fn fills_rules_and_strokes_are_told_apart() {
        let got = read(
            "0.12 0.31 0.47 rg 100 700 200 20 re f \
             0 g 100 680 200 0.75 re f \
             1 0 0 RG 2 w 50 600 m 250 600 l S \
             0 0 1 RG 1 w 300 500 50 40 re S",
        );
        assert_eq!(got.len(), 7, "{got:?}");
        assert_eq!(got[0], DrawnShape { rect: [100.0, 700.0, 300.0, 720.0], color: [0.12, 0.31, 0.47], rule: false, dot: false });
        assert!(got[1].rule && got[1].color == [0.0; 3], "a 0.75 pt fill is a rule");
        assert_eq!(got[2], DrawnShape { rect: [50.0, 599.0, 250.0, 601.0], color: [1.0, 0.0, 0.0], rule: true, dot: false }, "a 2 pt stroke");
        assert!(got[3..].iter().all(|s| s.rule && s.color == [0.0, 0.0, 1.0]), "a stroked box is its four sides");
    }

    #[test]
    fn the_ctm_places_shapes_and_slanted_ones_are_left_out() {
        let got = read("q 1 0 0 1 10 20 cm 0 g 0 0 100 50 re f Q q 0.7 0.7 -0.7 0.7 0 0 cm 0 0 10 10 re f Q 5 5 m 50 60 l S");
        assert_eq!(got, vec![DrawnShape { rect: [10.0, 20.0, 110.0, 70.0], color: [0.0; 3], rule: false, dot: false }]);
    }

    #[test]
    fn a_small_filled_circle_is_a_dot_and_a_large_one_is_nothing() {
        // A 4 pt disc drawn with four Béziers, as a browser draws a list bullet.
        let disc =
            "0 g 102 700 m 102 701.1 101.1 702 100 702 c 98.9 702 98 701.1 98 700 c 98 698.9 98.9 698 100 698 c 101.1 698 102 698.9 102 700 c f";
        let got = read(disc);
        assert_eq!(got, vec![DrawnShape { rect: [98.0, 698.0, 102.0, 702.0], color: [0.0; 3], rule: false, dot: true }]);
        let big = "0 g 200 700 m 200 760 150 800 100 800 c f";
        assert!(read(big).is_empty(), "a large curve is no dot");
    }

    #[test]
    fn hostile_input_is_bounded() {
        let many = "0 0 1 1 re ".repeat(MAX_SHAPES + 10) + "f";
        assert_eq!(read(&many).len(), MAX_SHAPES);
        assert!(read("1e400 0 0 1 0 0 cm 0 0 1 1 re f nan nan m 1 1 l S Q Q Q").is_empty());
        assert!(read("/Pattern cs /P1 scn 0 0 10 10 re f").is_empty(), "a pattern has no plain colour");
    }
}
