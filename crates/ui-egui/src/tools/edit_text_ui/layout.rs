use super::{Arc, BTreeMap, Color32, DocInfo, FontFamily, FontId, LineEditor, PageXform, Rect};

/// The Thai fonts that come with Windows and Office (lower case, as PDFs name them), which a
/// computer without them shows in TH Sarabun PSK (Thai edition).
const THAI_SYSTEM_FONTS: [&str; 10] =
    ["angsana", "browallia", "cordia", "dilleniaupc", "eucrosiaupc", "freesiaupc", "irisupc", "jasmineupc", "kodchiang", "lilyupc"];

/// The look shown for a paragraph: the family and weight guessed from its PDF font name.
pub(super) fn source_look(base_font: &str, size: f64, color: [f64; 3], detected_bold: bool, detected_italic: bool) -> pdfcraft_engine::AddedText {
    use pdfcraft_engine::FontFamily as F;
    let name = base_font.to_ascii_lowercase();
    let family = if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console"].iter().any(|s| name.contains(s)) {
        F::Courier
    } else if !name.contains("sans") && ["times", "serif", "roman", "cambria", "georgia", "palatino", "garamond"].iter().any(|s| name.contains(s)) {
        F::Times
    } else {
        F::Helvetica
    };
    // Thai edition: a Thai font of Windows and Office that this machine lacks stands in as TH
    // Sarabun PSK, which draws Thai, rather than the Latin guess.
    let ocr_layer = base_font == pdfcraft_fonts::thai_text::INVISIBLE_NAME;
    let family = if (ocr_layer || THAI_SYSTEM_FONTS.iter().any(|f| name.contains(f))) && F::Sarabun.craft(false, false).is_some() {
        F::Sarabun
    } else {
        family
    };
    // The font itself, when this build or this machine has it: the menu then names it, and text
    // typed in it previews in its own face.
    let family = F::like_font(base_font).unwrap_or(family);
    pdfcraft_engine::AddedText {
        family,
        bold: detected_bold || ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| name.contains(s)),
        italic: detected_italic || ["italic", "oblique", "slanted"].iter().any(|s| name.contains(s)),
        size: (size * 10.0).round() / 10.0,
        color,
        ..Default::default()
    }
}

pub(crate) fn look_of(b: &pdfcraft_engine::TextBlock) -> pdfcraft_engine::AddedText {
    // With the underline PdfCraft drew under it (Thai edition).
    pdfcraft_engine::AddedText { underline: b.underlined, ..source_look(&b.base_font, b.size, b.color, b.bold, b.italic) }
}

/// A paragraph of the page as an added text box with its text and look, over the paragraph
/// (display space), fitted to its rows (Thai edition: Copy and Ctrl+drag of existing text).
pub(crate) fn paragraph_as_added(xf: &PageXform, info: &DocInfo, page: usize, b: &pdfcraft_engine::TextBlock) -> Option<pdfcraft_engine::AddedText> {
    let p = info.pages.get(page)?;
    let r = xf.user_rect(info, page, b.rect.map(|v| v as f32));
    let ((x0, y0), (x1, y1)) = (xf.screen_to_view(r.left_bottom()), xf.screen_to_view(r.right_top()));
    let ph = f64::from(p.height);
    let text = pdfcraft_engine::AddedText {
        // Thai edition: the Windows Thai fonts' private-use glyph variants as the Thai they are,
        // or the box's own font can't draw them.
        text: pdfcraft_fonts::thai_text::from_presentation_forms(&b.editor_text).into_owned(),
        rect: [f64::from(x0), ph - f64::from(y0), f64::from(x1), ph - f64::from(y1)],
        ..look_of(b)
    };
    Some(pdfcraft_engine::fitted_text(&text, f64::from(p.width)))
}

pub(crate) fn editor_font(ctx: &egui::Context, look: &pdfcraft_engine::AddedText, size: f32) -> FontId {
    // A craft-fonts or installed family is shown in its own face, as it will be saved.
    if look.family.craft(look.bold, look.italic).is_some() {
        return crate::theme::document_font(ctx, look.family, look.bold, look.italic, size);
    }
    let family = match (look.family, look.bold) {
        (pdfcraft_engine::FontFamily::Courier, _) => FontFamily::Monospace,
        (_, true) => FontFamily::Name("semibold".into()),
        (_, false) => FontFamily::Proportional,
    };
    FontId::new(size, family)
}

pub(super) fn color32(color: [f64; 3]) -> Color32 {
    Color32::from_rgb(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// The dark box behind text too pale to read on white.
pub(super) const DARK_EDITOR_FILL: Color32 = Color32::from_gray(0x26);

/// WCAG relative luminance of an sRGB colour.
pub(super) fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio between two colours (1 to 21).
pub(super) fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The inline editor's background for text drawn in `text`: white, or a dark box when the text
/// reads better on that (white or pale text, which would vanish on white).
pub(crate) fn editor_fill(text: Color32) -> Color32 {
    if contrast(text, Color32::WHITE) >= contrast(text, DARK_EDITOR_FILL) { Color32::WHITE } else { DARK_EDITOR_FILL }
}

/// One TextEdit lays out all source runs; its caret and selection span the whole region.
pub(crate) fn install_source_fonts(ctx: &egui::Context, doc: &pdfcraft_engine::Document, page: usize) -> BTreeMap<String, String> {
    let programs = doc.text_editor_fonts(page);
    let mut definitions = ctx.fonts(|f| f.definitions().clone());
    // Only the current editor's document fonts live in the atlas. Never accumulate the
    // programs of previously opened files or add them to the interface's default families.
    definitions.font_data.retain(|name, _| !name.starts_with("pdf-source-"));
    definitions.families.retain(|family, _| !matches!(family, FontFamily::Name(name) if name.starts_with("pdf-source-")));
    let mut families = BTreeMap::new();
    for (index, program) in programs.iter().enumerate() {
        let name = format!("pdf-source-{index}");
        definitions.font_data.insert(name.clone(), Arc::new(egui::FontData::from_owned(program.bytes.clone())));
        families.insert(program.resource.clone(), name);
    }
    for program in &programs {
        let Some(name) = families.get(&program.resource) else { continue };
        // Word-created PDFs often split one face into several subsets. Use all of the
        // document's matching subsets before falling back for newly inserted characters.
        let mut stack = vec![name.clone()];
        stack.extend(
            programs
                .iter()
                .filter(|p| {
                    pdfcraft_fonts::embedding::face_key(&p.face) == pdfcraft_fonts::embedding::face_key(&program.face)
                        && p.resource != program.resource
                })
                .filter_map(|p| families.get(&p.resource).cloned()),
        );
        if let Some(fallback) = definitions.families.get(&FontFamily::Proportional) {
            stack.extend(fallback.iter().cloned());
        }
        definitions.families.insert(FontFamily::Name(name.clone().into()), stack);
    }
    if !families.is_empty() {
        ctx.set_fonts(definitions);
    }
    families
}

/// The editor's look, with what the pointer rests on in Format text (a font, a size, a colour, or
/// bold or italic turned the other way) standing in: a preview, nothing is applied until it is
/// clicked (Thai edition).
pub(super) fn shown_look(ctx: &egui::Context, ed: &LineEditor) -> pdfcraft_engine::AddedText {
    let mut look = ed.look.clone();
    if let Some(preview) = crate::content_ui::previewed_look(ctx) {
        preview.apply(&mut look);
    }
    look
}

/// The part of `text` a Format text preview applies to when it is not all of it: the selection,
/// while something is previewed (Thai edition).
pub(super) fn previewed_part(ctx: &egui::Context, ed: &LineEditor, text: &str) -> Option<std::ops::Range<usize>> {
    crate::content_ui::previewed_look(ctx)?;
    preview_range(ed.selection.as_ref(), text)
}

/// A selection that is a part of `text`: neither empty nor the whole of it, within its bounds.
pub(super) fn preview_range(selection: Option<&std::ops::Range<usize>>, text: &str) -> Option<std::ops::Range<usize>> {
    selection.filter(|r| r.start < r.end && r.end <= text.len() && !(r.start == 0 && r.end >= text.len())).cloned()
}

/// `spans` cut where `selection` starts and ends, each part with whether a preview shows on it:
/// all of them without a selection.
pub(super) fn preview_parts(
    spans: Vec<(std::ops::Range<usize>, usize)>,
    selection: Option<&std::ops::Range<usize>>,
) -> Vec<(std::ops::Range<usize>, usize, bool)> {
    let mut out = Vec::new();
    for (range, index) in spans {
        let mut bounds = vec![range.start];
        if let Some(sel) = selection {
            bounds.extend([sel.start, sel.end].into_iter().filter(|c| *c > range.start && *c < range.end));
        }
        bounds.push(range.end);
        bounds.dedup();
        for w in bounds.windows(2) {
            let (a, b) = (w[0], w[1]);
            out.push((a..b, index, selection.is_none_or(|sel| a >= sel.start && a < sel.end)));
        }
    }
    out
}

/// Thai edition: the parts of paragraph `block` (its lines `lines`) by colour and size, as
/// characters of its text, for a paste to keep; empty when it has one colour and one size.
pub(crate) fn paragraph_runs(block: &pdfcraft_engine::TextBlock, lines: &[pdfcraft_engine::TextLine]) -> Vec<pdfcraft_engine::TextRun> {
    let text = &block.editor_text;
    let chars = |at: usize| text.get(..at).map_or(0, |s| s.chars().count());
    let mut runs: Vec<pdfcraft_engine::TextRun> = Vec::new();
    for (range, index) in block.edited_spans(text) {
        let Some(line) = lines.get(index) else { continue };
        let span = chars(range.start)..chars(range.end);
        match runs.last_mut() {
            Some(last) if last.color == line.color && last.size == line.size && last.chars.end == span.start => last.chars.end = span.end,
            _ => runs.push(pdfcraft_engine::TextRun { chars: span, color: line.color, size: line.size }),
        }
    }
    let uniform = runs.windows(2).all(|w| w[0].color == w[1].color && w[0].size == w[1].size);
    if uniform { Vec::new() } else { runs }
}

/// Whether the editor shows its text underlined: as set, or as the pointer on Underline previews.
fn shown_underline(ctx: &egui::Context, ed: &LineEditor) -> bool {
    crate::content_ui::previewed_look(ctx).and_then(|p| p.underline).unwrap_or(ed.extras.underline)
}

/// The stroke that underlines editor text of `size` (screen points) in `color`.
fn underline_stroke(on: bool, size: f32, color: Color32) -> egui::Stroke {
    if on { egui::Stroke::new((size * 0.06).max(1.0), color) } else { egui::Stroke::NONE }
}

pub(super) fn editor_job(ui: &egui::Ui, ed: &LineEditor, text: &str, scale: f32, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let ed_look = shown_look(ui.ctx(), ed);
    let underline = shown_underline(ui.ctx(), ed);
    // An unchanged PDF row already has an authoritative line break. Do not rewrap it
    // because font rounding or a fallback face measures a fraction of a pixel wider.
    job.wrap.max_width = if text == ed.original && ed_look == ed.look0 && ed.extras == ed.extras0 && underline == ed.extras0.underline {
        f32::INFINITY
    } else {
        width
    };
    if let Some((block, lines)) = &ed.source {
        let lefts: Vec<_> = block.rows.iter().filter_map(|r| r.first().and_then(|i| lines.get(*i)).map(|l| l.rect[0])).collect();
        let left_span = lefts.iter().copied().fold(f64::NEG_INFINITY, f64::max) - lefts.iter().copied().fold(f64::INFINITY, f64::min);
        let centered = block.rows.len() > 1
            && left_span > block.size * 0.5
            && block.rows.iter().all(|row| {
                let rect = row
                    .iter()
                    .filter_map(|i| lines.get(*i))
                    .map(|l| l.rect)
                    .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
                rect.is_some_and(|r| ((r[0] + r[2] - block.rect[0] - block.rect[2]) / 2.0).abs() < block.size)
            });
        if centered {
            job.halign = egui::Align::Center;
        }
        // A preview over a selected part shows on that part only, as the change will (Thai edition):
        // each run is cut where the selection starts and ends.
        let previewed = previewed_part(ui.ctx(), ed, text);
        // The underline as the page draws it, part by part (Thai edition), and as previewed on
        // the selected part (or all of it).
        let previewed_underline = crate::content_ui::previewed_look(ui.ctx()).and_then(|p| p.underline);
        let unchanged = text == ed.original;
        let on_page = |range: &std::ops::Range<usize>| {
            if unchanged { super::underlined_at(block, text, range) } else { ed.extras.underline }
        };
        let cuts: Vec<usize> = if unchanged { block.underlined_parts.iter().flat_map(|p| [p.start, p.end]).collect() } else { Vec::new() };
        let pieces = preview_parts(block.edited_spans(text), previewed.as_ref()).into_iter().flat_map(|(range, index, inside)| {
            let mut bounds = vec![range.start];
            bounds.extend(cuts.iter().copied().filter(|c| *c > range.start && *c < range.end && text.is_char_boundary(*c)));
            bounds.push(range.end);
            bounds.sort_unstable();
            bounds.dedup();
            bounds.windows(2).map(|w| (w[0]..w[1], index, inside)).collect::<Vec<_>>()
        });
        for (range, index, inside) in pieces {
            let Some(line) = lines.get(index) else { continue };
            let ed_look = if inside { &ed_look } else { &ed.look };
            let mut look = source_look(&line.base_font, line.size, line.color, line.bold, line.italic);
            look.size = line.size;
            if ed_look.family != ed.look0.family || ed.family_chosen {
                look.family = ed_look.family;
            }
            if ed_look.bold != ed.look0.bold {
                look.bold = ed_look.bold;
            }
            if ed_look.italic != ed.look0.italic {
                look.italic = ed_look.italic;
            }
            if ed_look.color != ed.look0.color {
                look.color = ed_look.color;
            }
            if ed_look.size != ed.look0.size {
                look.size = ed_look.size;
            }
            let mut font = editor_font(ui.ctx(), &look, (look.size as f32 * scale).clamp(1.0, 300.0));
            let own_face = look.family.craft(look.bold, look.italic).is_some();
            let original_face =
                ed_look.family == ed.look0.family && !ed.family_chosen && ed_look.bold == ed.look0.bold && ed_look.italic == ed.look0.italic;
            let embedded = original_face && ed.fonts.contains_key(&line.font);
            if embedded && let Some(name) = ed.fonts.get(&line.font) {
                font.family = FontFamily::Name(name.clone().into());
            }
            let native_width = ui.fonts_mut(|f| f.layout_no_wrap(line.text.clone(), font.clone(), Color32::BLACK).size().x);
            let target_width = ((line.rect[2] - line.rect[0]) * look.size / line.size.max(0.01)) as f32 * scale;
            let spacing = if original_face { (target_width - native_width) / line.text.chars().count().saturating_sub(1).max(1) as f32 } else { 0.0 };
            let size = font.size;
            job.append(
                text.get(range.clone()).unwrap_or_default(),
                0.0,
                egui::TextFormat {
                    font_id: font,
                    extra_letter_spacing: spacing + ed.extras.char_spacing as f32 * scale,
                    color: color32(look.color),
                    italics: look.italic && !embedded && !own_face,
                    // A previewed underline shows under the selected part only, as it will be drawn.
                    underline: underline_stroke(
                        match previewed_underline {
                            Some(previewed) if inside => previewed,
                            _ => on_page(&range),
                        },
                        size,
                        color32(look.color),
                    ),
                    ..Default::default()
                },
            );
        }
    } else {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: editor_font(ui.ctx(), &ed_look, (ed_look.size as f32 * scale).clamp(1.0, 300.0)),
                color: color32(ed_look.color),
                italics: ed_look.italic && ed_look.family.craft(ed_look.bold, ed_look.italic).is_none(),
                underline: underline_stroke(underline, (ed_look.size as f32 * scale).clamp(1.0, 300.0), color32(ed_look.color)),
                ..Default::default()
            },
        );
    }
    job
}

/// `galley` drawn in no colour: its rows still place the caret and the selection.
pub(super) fn unseen(mut galley: Arc<egui::Galley>) -> Arc<egui::Galley> {
    for placed in &mut Arc::make_mut(&mut galley).rows {
        for vertex in &mut Arc::make_mut(&mut placed.row).visuals.mesh.vertices {
            vertex.color = Color32::TRANSPARENT;
        }
    }
    galley
}

/// Align the editable galley's baselines to the source rows instead of egui's interface
/// leading. Glyphs, meshes, caret and selection receive the same transform.
pub(super) fn editor_galley(ui: &egui::Ui, ed: &LineEditor, text: &str, scale: f32, width: f32) -> Arc<egui::Galley> {
    let mut job = editor_job(ui, ed, text, scale, width);
    let centered = job.halign == egui::Align::Center;
    job.halign = egui::Align::LEFT;
    let mut galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let Some((block, lines)) = &ed.source else { return galley };
    let sources: Vec<_> = block
        .rows
        .iter()
        .filter_map(|row| {
            let first = row.first().and_then(|i| lines.get(*i))?;
            let rect = row
                .iter()
                .filter_map(|i| lines.get(*i))
                .map(|l| l.rect)
                .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
            Some((first.baseline()[1], rect))
        })
        .collect();
    let Some((first_baseline, first_rect)) = sources.first().copied() else { return galley };
    let leading = if ed.extras.line_spacing > 0.0 {
        ed.extras.line_spacing * ed.look.size
    } else {
        let fallback = if block.lines.len() == 1 && !text.contains('\n') {
            block.lines.first().and_then(|i| lines.get(*i)).map_or(block.size * 1.2, |line| line.wrap_leading())
        } else {
            block.size * 1.5
        };
        sources.windows(2).map(|r| (r[0].0 - r[1].0).abs()).rfind(|v| *v > 0.0).unwrap_or(fallback)
    };
    let g = Arc::make_mut(&mut galley);
    let mut bounds = Rect::NOTHING;
    let mut meshes = Rect::NOTHING;
    // Which line of the text (between `\n`s) each row is on: rows aligned on their own go where
    // the rewrite will put them, in the paragraph's width.
    let mut line = 0usize;
    for (index, placed) in g.rows.iter_mut().enumerate() {
        let own = ed.look.aligns.get(line).copied().flatten();
        if placed.ends_with_newline {
            line = line.saturating_add(1);
        }
        let (last_baseline, last_rect) = sources.last().copied().unwrap_or((first_baseline, first_rect));
        let (baseline, source_rect) = sources
            .get(index)
            .copied()
            .unwrap_or((last_baseline - index.saturating_add(1).saturating_sub(sources.len()) as f64 * leading, last_rect));
        let changed_size = ed.look.size != ed.look0.size;
        let ratio = if changed_size { ed.look.size / ed.look0.size.max(0.01) } else { 1.0 };
        let ascent = (source_rect[3] - sources.get(index).map_or(last_baseline, |s| s.0)) as f32 * scale * ratio as f32;
        let descent = (sources.get(index).map_or(last_baseline, |s| s.0) - source_rect[1]) as f32 * scale * ratio as f32;
        let base_y = (block.rect[3] - baseline) as f32 * scale;
        let row = Arc::make_mut(&mut placed.row);
        placed.pos.y = base_y - ascent;
        let free = (width - row.size.x).max(0.0);
        placed.pos.x = match own {
            Some(pdfcraft_engine::TextAlign::Center) => free / 2.0,
            Some(pdfcraft_engine::TextAlign::Right) => free,
            Some(pdfcraft_engine::TextAlign::Left | pdfcraft_engine::TextAlign::Justify) => 0.0,
            None if centered => (width - row.size.x) / 2.0,
            None => (source_rect[0] - block.rect[0]) as f32 * scale,
        };
        for glyph in &mut row.glyphs {
            let dy = ascent - glyph.pos.y;
            glyph.pos.y = ascent;
            glyph.font_ascent = ascent;
            glyph.font_height = ascent + descent;
            glyph.line_height = ascent + descent;
            if !glyph.uv_rect.is_nothing() {
                for vertex in row.visuals.mesh.vertices.iter_mut().skip(glyph.first_vertex as usize).take(4) {
                    vertex.pos.y += dy;
                }
            }
        }
        row.size.y = (ascent + descent).max(1.0);
        row.visuals.mesh_bounds = row.visuals.mesh.vertices.iter().fold(Rect::NOTHING, |r, v| r.union(Rect::from_min_max(v.pos, v.pos)));
        bounds = bounds.union(Rect::from_min_size(placed.pos, row.size));
        meshes = meshes.union(row.visuals.mesh_bounds.translate(placed.pos.to_vec2()));
    }
    g.rect = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(bounds.right().max(0.0), bounds.bottom().max(0.0)));
    g.mesh_bounds = meshes;
    galley
}

/// Caret colour that stays visible on a field filled with `fill`: black on light fills, white on dark ones.
pub(crate) fn caret_color_on(fill: Color32) -> Color32 {
    let luma = 299 * u32::from(fill.r()) + 587 * u32::from(fill.g()) + 114 * u32::from(fill.b());
    if luma > 127_500 { Color32::BLACK } else { Color32::WHITE }
}
