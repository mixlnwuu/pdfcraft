//! Tables the page draws (Thai edition): a grid of rules is a table, its cells are the grid's
//! cells, and the borders, cell shading and row heights are the ones the page shows. Text-based
//! detection ([`crate::tables`]) only sees where text sits, so it misses a header row whose
//! labels are centred while the body is left aligned, and it can only guess at borders.

use super::layout::{Block, Cell, HAlign, Shape, Table, TableBorder};

/// How far apart (points) two rules may lie and still be one grid line.
const LINE_TOL: f64 = 1.5;

/// How far (points) a rule may stop short of another and still meet it.
const MEET: f64 = 2.5;

/// The most rules looked at on a page: grouping them is quadratic.
const MAX_RULES: usize = 2_000;

/// Rows past this make no table (a ruled form of many hundred lines stays paragraphs).
const MAX_ROWS: usize = 500;

struct Rule {
    /// Its line: y for a horizontal rule, x for a vertical one.
    at: f64,
    /// The span along it: [x0, x1] or [y0, y1].
    span: [f64; 2],
    thick: f64,
    color: [f64; 3],
}

/// The ruled tables of a page, top to bottom, and per block whether a table took it. Blocks
/// marked in `skip` are left alone.
pub(crate) fn ruled_tables(blocks: &[Block], shapes: &[Shape], skip: &[bool]) -> (Vec<Table>, Vec<bool>) {
    let mut consumed = vec![false; blocks.len()];
    let mut hs: Vec<Rule> = Vec::new();
    let mut vs: Vec<Rule> = Vec::new();
    for s in shapes.iter().filter(|s| s.rule && s.rect.iter().all(|v| v.is_finite())).take(MAX_RULES) {
        let [x0, y0, x1, y1] = s.rect;
        let (w, h) = (x1 - x0, y1 - y0);
        if w >= h && w >= 12.0 {
            hs.push(Rule { at: (y0 + y1) / 2.0, span: [x0, x1], thick: h.max(0.25), color: s.color });
        } else if h > w && h >= 8.0 {
            vs.push(Rule { at: (x0 + x1) / 2.0, span: [y0, y1], thick: w.max(0.25), color: s.color });
        }
    }
    if hs.len() < 2 || vs.len() < 2 {
        return (Vec::new(), consumed);
    }
    // Rules that meet are one grid: union-find over horizontal (0..n) and vertical (n..) rules.
    let n = hs.len();
    let mut parent: Vec<usize> = (0..n + vs.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while let Some(&up) = p.get(i) {
            if up == i {
                break;
            }
            let grand = p.get(up).copied().unwrap_or(up);
            if let Some(slot) = p.get_mut(i) {
                *slot = grand;
            }
            i = up;
        }
        i
    }
    for (hi, h) in hs.iter().enumerate() {
        for (vi, v) in vs.iter().enumerate() {
            let meets = v.at >= h.span[0] - MEET && v.at <= h.span[1] + MEET && h.at >= v.span[0] - MEET && h.at <= v.span[1] + MEET;
            if meets {
                let (a, b) = (find(&mut parent, hi), find(&mut parent, n + vi));
                if a != b
                    && let Some(slot) = parent.get_mut(a)
                {
                    *slot = b;
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, (Vec<usize>, Vec<usize>)> = std::collections::BTreeMap::new();
    for i in 0..n + vs.len() {
        let root = find(&mut parent, i);
        let g = groups.entry(root).or_default();
        if i < n { g.0.push(i) } else { g.1.push(i - n) }
    }
    let mut out = Vec::new();
    for (h_ids, v_ids) in groups.into_values() {
        let hr: Vec<&Rule> = h_ids.iter().filter_map(|i| hs.get(*i)).collect();
        let vr: Vec<&Rule> = v_ids.iter().filter_map(|i| vs.get(*i)).collect();
        if let Some(t) = grid_table(&hr, &vr, blocks, skip, &mut consumed) {
            out.push(t);
        }
    }
    out.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]));
    (out, consumed)
}

/// The distinct lines among `at` values, ascending.
fn lines(mut at: Vec<f64>) -> Vec<f64> {
    at.sort_by(f64::total_cmp);
    let mut out: Vec<f64> = Vec::new();
    for a in at {
        match out.last() {
            Some(l) if a - l <= LINE_TOL => {}
            _ => out.push(a),
        }
    }
    out
}

/// One grid's table, from its horizontal and vertical rules.
fn grid_table(hr: &[&Rule], vr: &[&Rule], blocks: &[Block], skip: &[bool], consumed: &mut [bool]) -> Option<Table> {
    // Top to bottom, and left to right.
    let mut ys = lines(hr.iter().map(|r| r.at).collect());
    ys.reverse();
    let xs = lines(vr.iter().map(|r| r.at).collect());
    let (rows, cols) = (ys.len().checked_sub(1)?, xs.len().checked_sub(1)?);
    // One box is a frame (a shaded note, a signature box), not a table.
    if rows * cols < 2 || cols > super::layout::MAX_COLS || rows > MAX_ROWS {
        return None;
    }
    let (left, right, top, bottom) = (*xs.first()?, *xs.last()?, *ys.first()?, *ys.last()?);
    // Whether a vertical rule runs along `x` through height `y` (a column boundary in that row).
    let wall = |x: f64, y: f64| vr.iter().any(|r| (r.at - x).abs() <= LINE_TOL && r.span[0] - MEET <= y && y <= r.span[1] + MEET);
    let mut cells: Vec<Vec<(usize, usize, Cell)>> = Vec::with_capacity(rows); // (first column, span, cell)
    for r in 0..rows {
        let (y_top, y_bottom) = (*ys.get(r)?, *ys.get(r + 1)?);
        let mid = (y_top + y_bottom) / 2.0;
        let mut row = Vec::new();
        let mut c = 0;
        while c < cols {
            let mut span = 1;
            while c + span < cols && !wall(*xs.get(c + span)?, mid) {
                span += 1;
            }
            row.push((
                c,
                span,
                Cell {
                    text: String::new(),
                    size: 11.0,
                    bold: false,
                    italic: false,
                    color: [0.0; 3],
                    span,
                    font: String::new(),
                    fill: None,
                    align: None,
                },
            ));
            c += span;
        }
        cells.push(row);
    }
    // Each block whose middle falls in a cell is that cell's text, in reading order.
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by(|a, b| blocks[*b].rect[3].total_cmp(&blocks[*a].rect[3]).then(blocks[*a].rect[0].total_cmp(&blocks[*b].rect[0])));
    let mut taken_any = false;
    for bi in order {
        let Some(b) = blocks.get(bi) else { continue };
        if skip.get(bi).copied().unwrap_or(false) || b.rotation.abs() > 1.0 || b.text.trim().is_empty() {
            continue;
        }
        let (cx, cy) = ((b.rect[0] + b.rect[2]) / 2.0, (b.rect[1] + b.rect[3]) / 2.0);
        if !(left < cx && cx < right && bottom < cy && cy < top) {
            continue;
        }
        let Some(r) = (0..rows).find(|r| ys.get(*r + 1).is_some_and(|lo| cy >= *lo)) else { continue };
        let Some(gc) = (0..cols).find(|c| xs.get(*c + 1).is_some_and(|hi| cx <= *hi)) else { continue };
        let Some((first, span, cell)) = cells.get_mut(r).and_then(|row| row.iter_mut().find(|(f, s, _)| *f <= gc && gc < f + s)) else { continue };
        if cell.text.is_empty() {
            let (x0, x1) = (xs.get(*first).copied().unwrap_or(left), xs.get(*first + *span).copied().unwrap_or(right));
            let (gap_l, gap_r) = (b.rect[0] - x0, x1 - b.rect[2]);
            let width = (x1 - x0).max(1.0);
            cell.align = Some(if gap_l > 3.0 && (gap_l - gap_r).abs() < width * 0.15 {
                HAlign::Center
            } else if gap_r < gap_l * 0.4 && gap_l > width * 0.3 {
                HAlign::Right
            } else {
                HAlign::Left
            });
            (cell.size, cell.bold, cell.italic, cell.color, cell.font) = (b.size, b.bold, b.italic, b.color, b.font.clone());
        } else {
            cell.text.push(' ');
        }
        cell.text.push_str(b.text.trim());
        if let Some(c) = consumed.get_mut(bi) {
            *c = true;
        }
        taken_any = true;
    }
    if !taken_any {
        return None;
    }
    // The border: the rules' commonest colour, at their usual thickness.
    let all: Vec<&&Rule> = hr.iter().chain(vr).collect();
    let mut thick: Vec<f64> = all.iter().map(|r| r.thick).collect();
    thick.sort_by(f64::total_cmp);
    let width = thick.get(thick.len() / 2).copied().unwrap_or(0.75);
    let color = all.iter().map(|r| r.color).max_by_key(|c| all.iter().filter(|r| r.color == *c).count()).unwrap_or([0.0; 3]);
    let row_heights = ys.windows(2).map(|w| w[0] - w[1]).collect();
    Some(Table {
        rect: [left, bottom, right, top],
        cols: xs.get(..cols)?.to_vec(),
        rows: cells.into_iter().map(|row| row.into_iter().map(|(_, _, c)| c).collect()).collect(),
        borders: Some(TableBorder { color, width }),
        row_heights,
        ruled: Some([true; 4]),
    })
}

/// The shading of each cell of `t`: the last filled area (drawn on top) whose box holds the
/// cell's middle and is no bigger than the table; white is no shading.
pub(crate) fn shade(t: &mut Table, shapes: &[Shape]) {
    let area = |r: &[f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    let table_area = area(&t.rect) * 1.05;
    let fills: Vec<&Shape> = shapes.iter().filter(|s| !s.rule && area(&s.rect) <= table_area).collect();
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let mut y = t.rect[3];
    for (r, row) in t.rows.iter_mut().enumerate() {
        let h = t.row_heights.get(r).copied().unwrap_or(0.0);
        let mid_y = y - h / 2.0;
        y -= h;
        let mut c = 0usize;
        for cell in row.iter_mut() {
            let (x0, x1) = (edges.get(c).copied().unwrap_or(t.rect[0]), edges.get(c + cell.span).copied().unwrap_or(t.rect[2]));
            let mid_x = (x0 + x1) / 2.0;
            cell.fill = fills
                .iter()
                .rev()
                .find(|s| s.rect[0] <= mid_x && mid_x <= s.rect[2] && s.rect[1] <= mid_y && mid_y <= s.rect[3])
                .map(|s| s.color)
                .filter(|c| c.iter().any(|v| *v < 0.98));
            c += cell.span;
        }
    }
}
