//! Bulleted lists (Thai edition). A paragraph whose lines have a bullet before them is a list:
//! one item per bullet, so a Word or HTML list can be made of it. The bullet is a glyph at the
//! start of the text (•, ◦, ▪, ●, ○, ■, □, -, –, —, ·, ‣, ⁃) or, as browsers and word processors
//! often draw it, a small filled dot just left of the line.

use super::layout::Block;

/// The glyphs that start a bulleted item.
const BULLETS: &[char] = &['•', '◦', '▪', '●', '○', '■', '□', '‣', '⁃', '·', '-', '–', '—'];

/// `text` without the bullet glyph and the space after it, when it starts with one.
fn unbulleted(text: &str) -> Option<&str> {
    let t = text.trim_start();
    let mut chars = t.chars();
    let first = chars.next()?;
    let rest = chars.as_str();
    (BULLETS.contains(&first) && rest.starts_with(char::is_whitespace)).then(|| rest.trim_start())
}

/// The list items `b` is, given its visual `rows` (top to bottom) and the dots the page draws
/// (`[x0, y0, x1, y1]`, the same space as `b.rect`): one block per bullet, marked as a bullet, or
/// `b` alone when it has none. A row without a bullet of its own continues the item above (a
/// long item that wraps).
pub fn list_items(b: Block, rows: &[String], dots: &[[f64; 4]]) -> Vec<Block> {
    let rows: Vec<&str> = rows.iter().map(|r| r.trim()).filter(|r| !r.is_empty()).collect();
    let size = b.size.max(1.0);
    let height = b.rect[3] - b.rect[1];
    if rows.is_empty() || !height.is_finite() || !size.is_finite() {
        return vec![b];
    }
    let row_h = height / rows.len() as f64;
    let row_top = |i: usize| b.rect[3] - row_h * i as f64;
    // Dots before the block's left edge, at the height of one of its rows, top to bottom.
    let mut beside: Vec<f64> = dots
        .iter()
        .filter(|d| d.iter().all(|v| v.is_finite()))
        .filter(|d| d[2] <= b.rect[0] + 1.0 && d[2] >= b.rect[0] - 4.0 * size)
        .map(|d| (d[1] + d[3]) / 2.0)
        .filter(|y| *y <= b.rect[3] + 1.0 && *y >= b.rect[1] - 1.0)
        .collect();
    beside.sort_by(|a, c| c.total_cmp(a));
    beside.dedup_by(|a, c| (*a - *c).abs() < row_h * 0.5);
    // Which rows start an item: the row each dot sits beside, or a row starting with a glyph.
    let mut starts = vec![false; rows.len()];
    for y in &beside {
        let i = ((b.rect[3] - y) / row_h).floor();
        // `i` is finite and not negative, so the cast saturates rather than wraps.
        if i.is_finite()
            && i >= 0.0
            && let Some(s) = starts.get_mut((i as usize).min(rows.len().saturating_sub(1)))
        {
            *s = true;
        }
    }
    let glyphs: Vec<bool> = rows.iter().map(|r| unbulleted(r).is_some()).collect();
    for (s, g) in starts.iter_mut().zip(&glyphs) {
        *s |= *g;
    }
    if !starts.iter().any(|s| *s) {
        return vec![b];
    }
    let mut out: Vec<Block> = Vec::new();
    // Rows above the first bullet stay a paragraph of their own (a lead-in).
    let mut current: Option<(Vec<String>, usize, bool)> = None; // (texts, first row, bulleted)
    let finish = |item: Option<(Vec<String>, usize, bool)>, end: usize, out: &mut Vec<Block>| {
        if let Some((texts, first, bullet)) = item {
            let rect = [b.rect[0], row_top(end), b.rect[2], row_top(first)];
            out.push(Block { text: texts.join(" "), rect, bullet, space_around: [false; 2], ..b.clone() });
        }
    };
    for (i, row) in rows.iter().enumerate() {
        let start = starts.get(i).copied().unwrap_or(false);
        let text = unbulleted(row).unwrap_or(row).to_string();
        match &mut current {
            Some((texts, _, _)) if !start => texts.push(text),
            _ => {
                finish(current.take(), i, &mut out);
                current = Some((vec![text], i, start));
            }
        }
    }
    finish(current, rows.len(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(rows: usize) -> Block {
        Block {
            text: String::new(),
            rect: [100.0, 600.0, 400.0, 600.0 + 20.0 * rows as f64],
            size: 16.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            space_around: [false; 2],
            font: String::new(),
            bullet: false,
        }
    }

    fn rows(r: &[&str]) -> Vec<String> {
        r.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_dot_beside_each_line_makes_one_item_per_line() {
        let dots = [[86.0, 648.0, 90.0, 652.0], [86.0, 628.0, 90.0, 632.0], [86.0, 608.0, 90.0, 612.0]];
        let items = list_items(para(3), &rows(&["First", "Second", "Third"]), &dots);
        assert_eq!(items.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["First", "Second", "Third"]);
        assert!(items.iter().all(|b| b.bullet));
        assert!(items[0].rect[3] > items[1].rect[3] && items[1].rect[3] > items[2].rect[3], "top to bottom");
    }

    #[test]
    fn a_wrapped_item_stays_one_item_and_a_lead_in_stays_a_paragraph() {
        let dots = [[86.0, 628.0, 90.0, 632.0]];
        let items = list_items(para(3), &rows(&["Do these:", "Change the battery before", "the end of the month"]), &dots);
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(!items[0].bullet && items[0].text == "Do these:");
        assert!(items[1].bullet && items[1].text == "Change the battery before the end of the month");
    }

    #[test]
    fn bullet_glyphs_count_and_plain_text_is_left_alone() {
        let items = list_items(para(2), &rows(&["• One", "- Two"]), &[]);
        assert_eq!(items.iter().map(|b| (b.text.as_str(), b.bullet)).collect::<Vec<_>>(), [("One", true), ("Two", true)]);
        let plain = list_items(para(2), &rows(&["-5 degrees", "Plain"]), &[]);
        assert_eq!(plain.len(), 1, "a minus sign without a space is no bullet");
        assert!(!plain[0].bullet);
        let far = list_items(para(1), &rows(&["Text"]), &[[10.0, 608.0, 14.0, 612.0]]);
        assert!(!far[0].bullet, "a dot far to the left is not this line's bullet");
    }
}
