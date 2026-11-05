//! Running headers and footers (Thai edition): text at the top or the bottom of every page, at
//! the same place, is the document's header or footer, not body text. Word gets it as its own
//! header and footer, with the page number and the page count as fields it fills in itself, so a
//! "Page 1 of 2" line stays right as the document grows.

use super::layout::{Block, Page};

/// The top and bottom bands: this share of the page's height, and at least this many points.
const BAND: f64 = 0.12;
const BAND_MIN: f64 = 48.0;

/// How far (points) the same header may move between pages.
const SAME_PLACE: f64 = 4.0;

/// The document's header and footer, and per page the blocks they take from its body.
pub(crate) struct Bands<'a> {
    /// The first page's header blocks, top to bottom then left to right.
    pub header: Vec<&'a Block>,
    pub footer: Vec<&'a Block>,
    /// The width of the page they come from, for placing them across it.
    pub width: f64,
    /// Per page, per block: a header or footer block.
    pub skip: Vec<Vec<bool>>,
}

/// A block's text with its digits as `#`: "Page 1 of 2" and "Page 2 of 2" are the same footer.
fn shape_of(text: &str) -> String {
    text.trim().chars().map(|c| if c.is_ascii_digit() || ('\u{0E50}'..='\u{0E59}').contains(&c) { '#' } else { c }).collect()
}

/// Whether `text` is a page-number line: a number, alone or with "page"/"of" words around it.
fn page_number(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    let digits = t.chars().any(|c| c.is_ascii_digit() || ('\u{0E50}'..='\u{0E59}').contains(&c));
    let words = t.chars().filter(|c| c.is_alphabetic()).count();
    digits && (words == 0 || t.contains("page") || t.contains("หน้า") || t.contains(" of ") || t.contains("จาก"))
}

/// Text in `band` (top when true) of every page at the same place is the header (footer). One
/// page has no other page to compare with: its footer is a page-number line only.
pub(crate) fn bands(pages: &[Page]) -> Bands<'_> {
    let mut skip: Vec<Vec<bool>> = pages.iter().map(|p| vec![false; p.blocks.len()]).collect();
    let in_band = |p: &Page, b: &Block, top: bool| {
        let band = (p.height * BAND).max(BAND_MIN);
        b.rotation.abs() <= 0.1 && if top { b.rect[1] >= p.height - band } else { b.rect[3] <= band }
    };
    let mut found = [Vec::new(), Vec::new()];
    for (k, top) in [true, false].into_iter().enumerate() {
        let Some(first) = pages.first() else { break };
        for (bi, b) in first.blocks.iter().enumerate() {
            if !in_band(first, b, top) || b.text.trim().is_empty() {
                continue;
            }
            let shape = shape_of(&b.text);
            // The block on each other page that repeats it, if every page has one.
            let mut twins = Vec::with_capacity(pages.len());
            for p in pages.get(1..).unwrap_or_default() {
                let twin = p.blocks.iter().position(|o| {
                    in_band(p, o, top)
                        && shape_of(&o.text) == shape
                        && (o.rect[0] - b.rect[0]).abs() <= SAME_PLACE.max(o.rect[2] - o.rect[0] - (b.rect[2] - b.rect[0]))
                        && (o.rect[1] - b.rect[1]).abs() <= SAME_PLACE
                });
                match twin {
                    Some(t) => twins.push(t),
                    None => break,
                }
            }
            let repeats = if pages.len() > 1 { twins.len() == pages.len() - 1 } else { !top && page_number(&b.text) };
            if !repeats {
                continue;
            }
            if let Some(s) = skip.get_mut(0).and_then(|s| s.get_mut(bi)) {
                *s = true;
            }
            for (pi, t) in twins.into_iter().enumerate() {
                if let Some(s) = skip.get_mut(pi + 1).and_then(|s| s.get_mut(t)) {
                    *s = true;
                }
            }
            if let Some(f) = found.get_mut(k) {
                f.push(b);
            }
        }
    }
    let [mut header, mut footer] = found;
    for list in [&mut header, &mut footer] {
        list.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]).then(a.rect[0].total_cmp(&b.rect[0])));
    }
    Bands { header, footer, width: pages.first().map_or(0.0, |p| p.width), skip }
}

/// A footer's text in pieces: plain text, the page number (`Some(true)`) and the page count
/// (`Some(false)`). The first number equal to `page` (1-based) is the page number, and a later
/// number equal to `pages` the count.
pub(crate) fn fields(text: &str, page: usize, pages: usize) -> Vec<(String, Option<bool>)> {
    let mut out: Vec<(String, Option<bool>)> = Vec::new();
    let (mut numbered, mut counted) = (false, false);
    let mut rest = text;
    while !rest.is_empty() {
        let digit = |c: char| c.is_ascii_digit() || ('\u{0E50}'..='\u{0E59}').contains(&c);
        let Some(start) = rest.find(digit) else {
            out.push((rest.to_string(), None));
            break;
        };
        let end = rest[start..].find(|c: char| !digit(c)).map_or(rest.len(), |e| start + e);
        let (before, number, after) = (&rest[..start], &rest[start..end], &rest[end..]);
        if !before.is_empty() {
            out.push((before.to_string(), None));
        }
        let value: Option<usize> = number
            .chars()
            .map(|c| c.to_digit(10).or_else(|| (c as u32).checked_sub(0x0E50)))
            .try_fold(0usize, |v, d| d.and_then(|d| v.checked_mul(10)?.checked_add(d as usize)));
        let kind = if !numbered && value == Some(page) {
            numbered = true;
            Some(true)
        } else if numbered && !counted && value == Some(pages) {
            counted = true;
            Some(false)
        } else {
            None
        };
        out.push((number.to_string(), kind));
        rest = after;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, x0: f64, y0: f64) -> Block {
        Block {
            text: text.into(),
            rect: [x0, y0, x0 + 120.0, y0 + 12.0],
            size: 12.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            space_around: [false; 2],
            font: String::new(),
            bullet: false,
        }
    }

    fn page(blocks: Vec<Block>) -> Page {
        Page { width: 595.0, height: 842.0, blocks, ..Default::default() }
    }

    #[test]
    fn repeated_top_and_bottom_text_is_the_header_and_footer() {
        let pages = vec![
            page(vec![block("Company report", 72.0, 800.0), block("Title", 72.0, 700.0), block("Page 1 of 2", 450.0, 30.0)]),
            page(vec![block("Company report", 72.0, 801.0), block("Body", 72.0, 700.0), block("Page 2 of 2", 450.0, 30.0)]),
        ];
        let b = bands(&pages);
        assert_eq!(b.header.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Company report"]);
        assert_eq!(b.footer.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Page 1 of 2"]);
        assert_eq!(b.skip, vec![vec![true, false, true], vec![true, false, true]]);
    }

    #[test]
    fn text_on_one_page_only_stays_in_the_body() {
        let pages = vec![page(vec![block("Big title", 72.0, 800.0), block("1", 290.0, 30.0)]), page(vec![block("Body", 72.0, 700.0)])];
        let b = bands(&pages);
        assert!(b.header.is_empty() && b.footer.is_empty(), "nothing repeats");
        // A single page: its page-number line is a footer, its top line is not a header.
        let one = vec![page(vec![block("Big title", 72.0, 800.0), block("หน้า 1 จาก 1", 450.0, 30.0)])];
        let b = bands(&one);
        assert!(b.header.is_empty());
        assert_eq!(b.footer.len(), 1);
    }

    #[test]
    fn page_numbers_become_fields() {
        assert_eq!(
            fields("หน้า 1 จาก 2", 1, 2),
            vec![("หน้า ".into(), None), ("1".into(), Some(true)), (" จาก ".into(), None), ("2".into(), Some(false))]
        );
        assert_eq!(fields("- 1 -", 1, 1), vec![("- ".into(), None), ("1".into(), Some(true)), (" -".into(), None)]);
        assert_eq!(fields("Report 2024", 1, 3), vec![("Report ".into(), None), ("2024".into(), None)]);
        assert_eq!(fields("หน้า ๓", 3, 9), vec![("หน้า ".into(), None), ("๓".into(), Some(true))]);
        assert!(fields("", 1, 1).is_empty());
    }
}
