//! Thai edition: the invisible text layer for pages with Thai words. Upstream's layer stretches
//! each word over its box in blank Type3 glyphs, so a Thai word opened for editing came out tiny
//! and squeezed; here a word is set at its box's height in [`INVISIBLE_FONT`] (an em wide per
//! character, its marks counted in its height), its characters moved apart to fill the word
//! (`TJ`), so it keeps its proportions when edited (and is then drawn in TH Sarabun PSK).

use super::{PlacedWord, helvetica_width, ink_extent, num};

/// The resource name of the invisible font a text layer with Thai uses
/// ([`pdfcraft_fonts::thai_text::invisible_font`]).
pub const INVISIBLE_FONT: &[u8] = b"PCOcrUni";

/// Whether a layer of `words` takes [`thai_text_layer`]: any word has Thai in it.
pub fn has_thai(words: &[PlacedWord]) -> bool {
    words.iter().any(|w| w.text.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c)))
}

/// [`ink_extent`] for Thai, as TH Sarabun PSK draws it: letters a little under half the em, tall
/// vowels above them, tone marks and upper vowels higher, lower vowels and tails below.
fn thai_ink_extent(text: &str) -> (f64, f64) {
    let has = |set: &[char]| text.chars().any(|c| set.contains(&c));
    let marks = ['\u{0E31}', '\u{0E34}', '\u{0E35}', '\u{0E36}', '\u{0E37}', '\u{0E47}', '\u{0E48}', '\u{0E49}', '\u{0E4A}', '\u{0E4B}', '\u{0E4C}'];
    let tall = ['\u{0E40}', '\u{0E41}', '\u{0E42}', '\u{0E43}', '\u{0E44}', '\u{0E1B}', '\u{0E1D}', '\u{0E1F}'];
    let low = ['\u{0E38}', '\u{0E39}', '\u{0E3A}', '\u{0E0E}', '\u{0E0F}', '\u{0E24}', '\u{0E26}'];
    let top = if has(&marks) {
        0.76
    } else if has(&tall) {
        0.62
    } else {
        0.46
    };
    (top, if has(&low) { -0.16 } else { 0.0 })
}

/// Page content that writes `words` as invisible text: WinAnsi words in `/PCHelv` (standard
/// Helvetica) stretched to their boxes, others (Thai) in [`INVISIBLE_FONT`] at their box's
/// height, each character moved to fill the word. Marked content `/OCR`, as upstream's layer.
pub fn thai_text_layer(words: &[PlacedWord]) -> Vec<u8> {
    // `3 Tr` and the font are graphics state, which `ET` doesn't reset: without `q … Q` they
    // would stay set for every later content stream on the page, so text added after OCR (a
    // header or footer) drew invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\n".to_vec();
    let mut wide = false;
    for w in words {
        let unicode = pdfcraft_fonts::first_undrawable(&w.text).is_some();
        let height = w.up[0].hypot(w.up[1]);
        // The em square: the word's own ink (cap height or x-height, descenders or not) spans the
        // box, so the text is as tall as the scanned word; its width is stretched to fit the word.
        let (top, bottom) = if unicode { thai_ink_extent(&w.text) } else { ink_extent(&w.text) };
        let em = [w.up[0] / (top - bottom), w.up[1] / (top - bottom)];
        // How wide the word is, in the size it is written at.
        let width = if unicode { w.across[0].hypot(w.across[1]) / (height / (top - bottom)) } else { helvetica_width(&w.text, 1.0) };
        if !(width > 0.0 && height > 0.0) {
            continue;
        }
        if unicode != wide {
            wide = unicode;
            let name = if wide { String::from_utf8_lossy(INVISIBLE_FONT).into_owned() } else { "PCHelv".to_owned() };
            out.extend(format!("/{name} 1 Tf\n").bytes());
        }
        let ax = [w.across[0] / width, w.across[1] / width];
        let o = [w.origin[0] - em[0] * bottom, w.origin[1] - em[1] * bottom];
        let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
        let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
        out.extend_from_slice(nums.join(" ").as_bytes());
        out.extend_from_slice(b" Tm ");
        if wide {
            out.extend(spread(&w.text, width).bytes());
        } else {
            out.extend(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)));
            out.extend_from_slice(b" Tj\n");
        }
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    out
}

/// A Thai mark drawn over or under the letter before it, taking no width of its own.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

/// Where each character of `text` starts along a word `width` ems wide: by its advance in TH
/// Sarabun PSK, marks at the start of the letter they sit on, scaled to the word (Thai edition).
/// Without that face, each letter gets an equal share and the marks none.
fn positions(text: &str, width: f64) -> Vec<f64> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let natural = |end: usize| pdfcraft_fonts::thai_text::line_width(text.get(..end).unwrap_or(""), 1.0, false, false, &helvetica_width);
    let total = natural(text.len());
    let shaped = total.is_finite() && total > 0.0;
    let letters = chars.iter().filter(|(_, c)| !is_mark(*c)).count().max(1) as f64;
    let mut out = Vec::with_capacity(chars.len());
    let (mut base, mut count) = (0.0, 0.0);
    for (at, c) in &chars {
        if is_mark(*c) {
            out.push(base);
            continue;
        }
        base = if shaped { natural(*at) / total * width } else { count / letters * width };
        count += 1.0;
        out.push(base);
    }
    out
}

/// The `TJ` that sets `text` in [`INVISIBLE_FONT`] (one em per character code) across a word
/// `width` ems wide: each character where [`positions`] puts it, and the pen brought back to the
/// word's end, which is where the reading of the run (its box) ends. No horizontal scaling (`Tz`):
/// a word opened for editing takes the run's scale, and its new text came out squeezed and small.
fn spread(text: &str, width: f64) -> String {
    let at = positions(text, width);
    let mut ends: Vec<f64> = at.iter().skip(1).copied().collect();
    ends.push(width);
    let mut parts = Vec::new();
    for (c, (from, to)) in text.chars().zip(at.iter().zip(&ends)) {
        let mut units = [0u16; 2];
        let codes: String = c.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
        parts.push(format!("<{codes}>"));
        // Each code is one em: thousandths of an em back (or on), for the next to start where it
        // should.
        let pull = (1.0 - (to - from).max(0.0)) * 1000.0;
        parts.push(num(if pull.is_finite() { pull } else { 0.0 }));
    }
    format!(
        "[{}] TJ
",
        parts.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thai edition: marks take no width and sit at the start of their letter; the word is spread
    /// over its whole width, with no gap that reads as a space, and the pen ends at the word's end.
    #[test]
    fn letters_take_their_own_width_and_marks_none() {
        let text = "\u{0E1C}\u{0E39}\u{0E49}\u{0E21}"; // ผู้ม
        let at = positions(text, 4.0);
        assert_eq!(at.len(), 4);
        assert_eq!(at[0], 0.0);
        assert_eq!((at[1], at[2]), (at[0], at[0]), "the marks are on the first letter: {at:?}");
        assert!(at[3] > 0.0 && at[3] < 4.0, "{at:?}");
        let tj = spread(text, 4.0);
        assert!(tj.starts_with('[') && tj.ends_with("] TJ\n") && !tj.contains("Tz"), "no horizontal scaling: {tj}");
        let inner = tj.split(['[', ']']).nth(1).unwrap();
        let pulls: Vec<f64> = inner.split(' ').filter(|p| !p.starts_with('<')).map(|p| p.parse().unwrap()).collect();
        let end: f64 = pulls.iter().map(|p| 1.0 - p / 1000.0).sum();
        assert!((end - 4.0).abs() < 1e-3, "ends at the word's end: {end}");
    }
}
