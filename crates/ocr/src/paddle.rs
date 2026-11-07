//! Thai edition: Thai text recognition with PaddleOCR's models (Apache-2.0): PP-OCRv3 text
//! detection and PP-OCRv5 Thai recognition, in ONNX form, run in this process by the same engine
//! as the Latin models (rten). No page leaves the machine.
//!
//! Detection gives each pixel a probability of being text: the regions above [`BINARY`] are the
//! lines. Recognition reads one line at a time: each time step of its output is a character or
//! the CTC blank, repeats merged.

use std::path::{Path, PathBuf};

use rten::Model;
use rten_tensor::prelude::*;
use rten_tensor::{NdTensor, NdTensorView};

use crate::{Line, OcrError, Word};

pub const DETECTION_MODEL: &str = "paddle-text-detection.onnx";
pub const RECOGNITION_MODEL: &str = "paddle-thai-recognition.onnx";
pub const DICTIONARY: &str = "paddle-thai-dict.txt";

/// Longest side, in pixels, of the picture detection sees; a side is a multiple of [`STRIDE`].
const MAX_SIDE: usize = 1920;
const STRIDE: usize = 32;
/// A pixel is text from this probability.
const BINARY: f32 = 0.3;
/// A region is a line of text from this mean probability.
const REGION: f32 = 0.5;
/// How far a region is grown, as PaddleOCR's `unclip_ratio`: the model finds a line's core.
const UNCLIP: f32 = 1.6;
/// The recognition model's line height, and the widest line it is given.
const LINE_HEIGHT: usize = 48;
const MAX_LINE_WIDTH: usize = 3200;
/// Most regions read on one page.
const MAX_REGIONS: usize = 4000;

/// Where the Thai model files are.
#[derive(Clone, Debug, PartialEq)]
pub struct ThaiModels {
    pub detection: PathBuf,
    pub recognition: PathBuf,
    pub dictionary: PathBuf,
}

impl ThaiModels {
    /// The Thai models in `dir`, when all three are there.
    pub fn in_dir(dir: &Path) -> Option<ThaiModels> {
        let models = ThaiModels { detection: dir.join(DETECTION_MODEL), recognition: dir.join(RECOGNITION_MODEL), dictionary: dir.join(DICTIONARY) };
        (models.detection.is_file() && models.recognition.is_file() && models.dictionary.is_file()).then_some(models)
    }
}

/// A loaded Thai recogniser.
pub struct ThaiOcr {
    detection: Model,
    recognition: Model,
    /// The characters by class; class 0 is the CTC blank.
    alphabet: Vec<String>,
}

fn failed(e: &dyn std::fmt::Display) -> OcrError {
    OcrError::Recognize(e.to_string())
}

impl ThaiOcr {
    pub fn load(models: &ThaiModels) -> Result<ThaiOcr, OcrError> {
        let load = |p: &Path| Model::load_file(p).map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()));
        let text = std::fs::read_to_string(&models.dictionary).map_err(|e| OcrError::Load(models.dictionary.display().to_string(), e.to_string()))?;
        Ok(ThaiOcr { detection: load(&models.detection)?, recognition: load(&models.recognition)?, alphabet: alphabet(&text) })
    }

    /// Recognise the text in an RGBA, RGB or grey image.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        let (w, h) = (width as usize, height as usize);
        let area = w.checked_mul(h).filter(|a| *a > 0).ok_or(OcrError::EmptyImage)?;
        let channels = [4, 3, 1].into_iter().find(|c| area.checked_mul(*c) == Some(pixels.len())).ok_or(OcrError::EmptyImage)?;
        // One byte order for everything after this: the models were trained on BGR pictures.
        let bgr = |x: usize, y: usize| -> [u8; 3] {
            let at = (y * w + x) * channels;
            match pixels.get(at..at + channels) {
                Some([grey]) => [*grey; 3],
                Some([r, g, b, ..]) => [*b, *g, *r],
                _ => [255; 3],
            }
        };
        let mut read = Vec::new();
        for region in self.regions(&bgr, w, h)? {
            if let Some(line) = self.read_line(&bgr, region)? {
                read.push(line);
            }
        }
        Ok(reading_order(read))
    }

    /// The lines of text on the page, as boxes in image pixels `[left, top, right, bottom]`.
    fn regions(&self, bgr: &dyn Fn(usize, usize) -> [u8; 3], w: usize, h: usize) -> Result<Vec<[f32; 4]>, OcrError> {
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        let scale = (MAX_SIDE as f32 / w.max(h) as f32).min(1.0);
        let side = |v: usize| (((v as f32 * scale) as usize).div_ceil(STRIDE) * STRIDE).max(STRIDE);
        let (sw, sh) = (side(w), side(h));
        let mut input = NdTensor::<f32, 4>::zeros([1, 3, sh, sw]);
        for y in 0..sh {
            let from_y = (y * h / sh).min(h - 1);
            for x in 0..sw {
                let px = bgr((x * w / sw).min(w - 1), from_y);
                for c in 0..3 {
                    input[[0, c, y, x]] = (f32::from(px[c]) / 255.0 - MEAN[c]) / STD[c];
                }
            }
        }
        let output: NdTensor<f32, 4> =
            self.detection.run_one(input.view().into(), None).map_err(|e| failed(&e))?.try_into().map_err(|e| failed(&e))?;
        let [_, _, oh, ow] = output.shape();
        let map = |x: usize, y: usize| output[[0, 0, y, x]];
        let (sx, sy) = (w as f32 / ow as f32, h as f32 / oh as f32);
        Ok(components(&map, ow, oh)
            .into_iter()
            .take(MAX_REGIONS)
            .map(|[l, t, r, b]| {
                let (bw, bh) = (r - l, b - t);
                let grow = (bw * bh * UNCLIP / (2.0 * (bw + bh)).max(1.0)).max(1.0);
                [((l - grow) * sx).max(0.0), ((t - grow) * sy).max(0.0), ((r + grow) * sx).min(w as f32), ((b + grow) * sy).min(h as f32)]
            })
            .collect())
    }

    /// The text of one line.
    fn read_line(&self, bgr: &dyn Fn(usize, usize) -> [u8; 3], [left, top, right, bottom]: [f32; 4]) -> Result<Option<Line>, OcrError> {
        let (bw, bh) = (right - left, bottom - top);
        if bw < 2.0 || bh < 2.0 {
            return Ok(None);
        }
        let width = ((bw / bh * LINE_HEIGHT as f32).ceil() as usize).clamp(LINE_HEIGHT / 2, MAX_LINE_WIDTH);
        let mut input = NdTensor::<f32, 4>::zeros([1, 3, LINE_HEIGHT, width]);
        for y in 0..LINE_HEIGHT {
            let from_y = (top + (y as f32 + 0.5) * bh / LINE_HEIGHT as f32) as usize;
            for x in 0..width {
                let px = bgr((left + (x as f32 + 0.5) * bw / width as f32) as usize, from_y);
                for c in 0..3 {
                    input[[0, c, y, x]] = (f32::from(px[c]) / 255.0 - 0.5) / 0.5;
                }
            }
        }
        let output: NdTensor<f32, 3> =
            self.recognition.run_one(input.view().into(), None).map_err(|e| failed(&e))?.try_into().map_err(|e| failed(&e))?;
        let picks = best_classes(output.view());
        let mut words = words_of(&decode(&picks, &self.alphabet), picks.len(), [left, top, right, bottom]);
        for word in &mut words {
            word.rect = ink_rows(bgr, word.rect);
        }
        Ok((!words.is_empty()).then_some(Line { words }))
    }
}

/// `rect` with its top and bottom on the ink inside it. A region is grown past its text, and the
/// text layer sizes a word by the height of its box. A row dark nearly all the way across is a
/// ruled line, not text. A box with no contrast stays as it is.
fn ink_rows(bgr: &dyn Fn(usize, usize) -> [u8; 3], rect @ [left, top, right, bottom]: [f32; 4]) -> [f32; 4] {
    let (x0, x1, y0, y1) = (left as usize, right.ceil() as usize, top as usize, bottom.ceil() as usize);
    let width = x1.saturating_sub(x0);
    if width == 0 || y1 <= y0 {
        return rect;
    }
    let grey = |x: usize, y: usize| bgr(x, y).iter().map(|c| u32::from(*c)).sum::<u32>() / 3;
    let (mut low, mut high) = (u32::MAX, 0);
    for y in y0..y1 {
        for x in x0..x1 {
            let v = grey(x, y);
            (low, high) = (low.min(v), high.max(v));
        }
    }
    if high < low + 40 {
        return rect;
    }
    let dark = (low + high) / 2;
    let inked = |y: usize| {
        let count = (x0..x1).filter(|x| grey(*x, y) < dark).count();
        count >= 2 && count * 100 < width * 85
    };
    match ((y0..y1).find(|y| inked(*y)), (y0..y1).rev().find(|y| inked(*y))) {
        (Some(first), Some(last)) => [left, first as f32, right, (last + 1) as f32],
        _ => rect,
    }
}

/// The classes the dictionary names, by class number: class 0 is the blank, then the dictionary's
/// characters, then the space.
fn alphabet(dictionary: &str) -> Vec<String> {
    let mut classes = vec![String::new()];
    classes.extend(dictionary.lines().map(|l| l.trim_end_matches('\r').to_owned()));
    classes.push(" ".to_owned());
    classes
}

/// The likeliest class at each time step of a `[1, steps, classes]` output.
fn best_classes(output: NdTensorView<f32, 3>) -> Vec<usize> {
    let [_, steps, classes] = output.shape();
    (0..steps)
        .map(|t| (0..classes).fold((0, f32::NEG_INFINITY), |best, c| if output[[0, t, c]] > best.1 { (c, output[[0, t, c]]) } else { best }).0)
        .collect()
}

/// CTC decoding: a run of one class is one character, the blank is none. Each character comes
/// with the time step it starts at.
fn decode(picks: &[usize], alphabet: &[String]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut previous = 0;
    for (t, &class) in picks.iter().enumerate() {
        if class != previous
            && class != 0
            && let Some(text) = alphabet.get(class)
        {
            out.push((t, text.clone()));
        }
        previous = class;
    }
    out
}

/// The characters of a line as words, split at its spaces, each with its share of the line's box
/// by the time steps it spans.
fn words_of(chars: &[(usize, String)], steps: usize, [left, top, right, bottom]: [f32; 4]) -> Vec<Word> {
    let x = |t: usize| left + (right - left) * t as f32 / steps.max(1) as f32;
    let mut words = Vec::new();
    let mut text = String::new();
    let mut start = 0;
    for (i, (t, c)) in chars.iter().enumerate() {
        if c == " " {
            if !text.is_empty() {
                words.push(Word::new(std::mem::take(&mut text), [x(start), top, x(*t), bottom], "paddle"));
            }
            continue;
        }
        if text.is_empty() {
            start = *t;
        }
        text.push_str(c);
        if i + 1 == chars.len() {
            words.push(Word::new(std::mem::take(&mut text), [x(start), top, right, bottom], "paddle"));
        }
    }
    words
}

/// Boxes `[left, top, right, bottom]` of the 4-connected regions of `map` above [`BINARY`] whose
/// mean is at least [`REGION`], a little taller than a speck.
fn components(map: &dyn Fn(usize, usize) -> f32, w: usize, h: usize) -> Vec<[f32; 4]> {
    let mut seen = vec![false; w * h];
    let mut boxes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        let (sx, sy) = (start % w, start / w);
        if seen[start] || map(sx, sy) < BINARY {
            continue;
        }
        seen[start] = true;
        stack.push((sx, sy));
        let (mut l, mut t, mut r, mut b) = (sx, sy, sx, sy);
        let (mut sum, mut count) = (0.0f32, 0u32);
        while let Some((x, y)) = stack.pop() {
            sum += map(x, y);
            count += 1;
            (l, t, r, b) = (l.min(x), t.min(y), r.max(x), b.max(y));
            let mut visit = |nx: usize, ny: usize| {
                let at = ny * w + nx;
                if !seen[at] && map(nx, ny) >= BINARY {
                    seen[at] = true;
                    stack.push((nx, ny));
                }
            };
            if x > 0 {
                visit(x - 1, y);
            }
            if x + 1 < w {
                visit(x + 1, y);
            }
            if y > 0 {
                visit(x, y - 1);
            }
            if y + 1 < h {
                visit(x, y + 1);
            }
        }
        if count >= 12 && sum / count as f32 >= REGION {
            boxes.push([l as f32, t as f32, r as f32 + 1.0, b as f32 + 1.0]);
        }
    }
    boxes
}

/// Lines top to bottom, those side by side on one row left to right.
fn reading_order(mut lines: Vec<Line>) -> Vec<Line> {
    let rect = |l: &Line| l.words.first().map_or([0.0; 4], |w| w.rect);
    lines.sort_by(|a, b| rect(a)[1].total_cmp(&rect(b)[1]));
    let mut rows: Vec<Vec<Line>> = Vec::new();
    for line in lines {
        let r = rect(&line);
        let middle = (r[1] + r[3]) / 2.0;
        match rows.last_mut().filter(|row| row.first().is_some_and(|f| middle < rect(f)[3])) {
            Some(row) => row.push(line),
            None => rows.push(vec![line]),
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| rect(a)[0].total_cmp(&rect(b)[0]));
    }
    rows.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dictionary_sits_between_the_blank_and_the_space() {
        let classes = alphabet("\u{0E01}\r\n\u{0E02}\n");
        assert_eq!(classes, ["", "\u{0E01}", "\u{0E02}", " "]);
    }

    /// CTC: repeats are one character, a blank between them makes two, the blank is nothing, and a
    /// class past the alphabet is skipped.
    #[test]
    fn repeats_merge_and_blanks_vanish() {
        let classes = alphabet("a\nb\n");
        let text = |picks: &[usize]| decode(picks, &classes).into_iter().map(|(_, c)| c).collect::<String>();
        assert_eq!(text(&[1, 1, 0, 1, 2, 2, 0, 0, 99, 3, 3, 2]), "aab b");
        assert_eq!(text(&[]), "");
        assert_eq!(text(&[0, 0]), "");
    }

    /// Words share the line's box by the time steps they span, and spaces only separate.
    #[test]
    fn a_line_splits_at_its_spaces() {
        let chars: Vec<(usize, String)> = [(0, "a"), (2, "b"), (5, " "), (6, " "), (7, "c")].map(|(t, c)| (t, c.to_owned())).to_vec();
        let words = words_of(&chars, 10, [0.0, 10.0, 100.0, 30.0]);
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["ab", "c"]);
        assert_eq!(words[0].rect, [0.0, 10.0, 50.0, 30.0]);
        assert_eq!(words[1].rect, [70.0, 10.0, 100.0, 30.0]);
        assert!(words_of(&[(0, " ".to_owned())], 4, [0.0; 4]).is_empty());
        assert!(words_of(&[], 0, [0.0; 4]).is_empty());
    }

    /// Two regions are found apart, a speck and faint pixels are not lines.
    #[test]
    fn regions_are_connected_probable_pixels() {
        let (w, h) = (20, 6);
        let map = |x: usize, y: usize| match (x, y) {
            (1..=6, 1..=2) | (10..=15, 3..=4) => 0.9,
            (18, 5) => 0.9,
            (0, 5) => 0.2,
            _ => 0.0,
        };
        assert_eq!(components(&map, w, h), [[1.0, 1.0, 7.0, 3.0], [10.0, 3.0, 16.0, 5.0]]);
        assert!(components(&|_, _| 0.0, 0, 0).is_empty());
    }

    #[test]
    fn lines_read_top_to_bottom_then_left_to_right() {
        let line = |text: &str, rect: [f32; 4]| Line { words: vec![Word::new(text, rect, "paddle")] };
        let order = reading_order(vec![
            line("below", [0.0, 50.0, 100.0, 70.0]),
            line("right", [200.0, 11.0, 300.0, 31.0]),
            line("left", [0.0, 10.0, 100.0, 30.0]),
        ]);
        assert_eq!(order.iter().map(Line::text).collect::<Vec<_>>(), ["left", "right", "below"]);
    }

    /// A word's box ends at its ink; the ruled line it sits on, and a blank box, don't count.
    #[test]
    fn a_box_shrinks_to_its_ink_and_past_a_ruled_line() {
        let page = |x: usize, y: usize| -> [u8; 3] {
            match (x, y) {
                (_, 18) => [0; 3],
                (10..=30, 6..=12) => [0; 3],
                _ => [255; 3],
            }
        };
        assert_eq!(ink_rows(&page, [0.0, 0.0, 50.0, 25.0]), [0.0, 6.0, 50.0, 13.0]);
        assert_eq!(ink_rows(&|_, _| [255; 3], [0.0, 2.0, 50.0, 25.0]), [0.0, 2.0, 50.0, 25.0]);
        assert_eq!(ink_rows(&page, [5.0, 5.0, 5.0, 5.0]), [5.0, 5.0, 5.0, 5.0]);
    }

    /// A picture that isn't one of the sizes given is refused, not read past its end.
    #[test]
    fn a_short_picture_is_refused_before_the_models() {
        let Some(models) = crate::Models::search_dirs().iter().find_map(|d| ThaiModels::in_dir(d)) else { return };
        let ocr = ThaiOcr::load(&models).unwrap();
        assert!(matches!(ocr.recognize(&[0; 5], 10, 10), Err(OcrError::EmptyImage)));
        assert!(matches!(ocr.recognize(&[], 0, 0), Err(OcrError::EmptyImage)));
    }
}
