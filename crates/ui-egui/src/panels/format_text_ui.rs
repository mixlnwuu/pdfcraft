//! Format text, laid out as Acrobat's panel: the font across the panel; size, colour, lists and
//! alignment; bold, italic, underline, superscript and subscript; line and paragraph spacing;
//! horizontal scale and character spacing. One panel serves a paragraph being edited (with the
//! fonts already on its page and the spacing controls) and added text (the standard families,
//! size, colour, style with underline, alignment, line spacing and character spacing). What PdfCraft can't apply yet (lists,
//! paragraph spacing, or superscript, subscript and scale for added text) is shown
//! disabled and says so.
//!
//! The icons are Lucide's (AGENTS.md §1); only the arrangement follows Acrobat.

use crate::widgets::Tip as _;
use egui::{Align, Color32, CornerRadius, Layout, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::text_script::Script;
use pdfcraft_engine::{AddedText, FontFamily, TextAlign};

use crate::edit_text_ui::Extras;
use crate::theme::Tokens;
use crate::{icons, widgets};

/// A font picked from the font list. The caller applies it, since keeping the paragraph's own
/// font also means restoring the family it started with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FontChoice {
    /// The fonts the paragraph already uses.
    KeepSource,
    /// One of the page's fonts, by resource name.
    Resource(String),
    /// A standard family.
    Family(FontFamily),
    /// Load font…: pick a font file.
    Load,
    /// A font file installed on this computer.
    Installed(std::path::PathBuf),
}

/// What the font list offers and shows.
pub(crate) struct Fonts<'a> {
    /// The fonts on the paragraph's page (resource, face); `None` for added text, which has the
    /// standard families only.
    pub sources: Option<&'a [(String, String)]>,
    /// The name the list shows for the current choice.
    pub shown: String,
    pub mixed_size: bool,
    /// The text's direction can be chosen (Thai edition): added text, and selected boxes.
    pub directions: bool,
}

/// Text being typed in, for the alignment buttons: with some of it selected, they show and set the
/// alignment of the paragraphs (lines between `\n`s) the selection touches; with none, the box's.
pub(crate) struct Paragraphs<'a> {
    pub text: &'a str,
    /// A byte range of `text`; empty or `None` is nothing selected.
    pub selection: Option<std::ops::Range<usize>>,
    /// The box's own alignment is known (added text), so a paragraph set to it keeps no entry of its
    /// own. An edited paragraph's alignment is only guessed from where its rows sit: its rows keep
    /// every alignment set.
    pub tidy: bool,
}

impl Paragraphs<'_> {
    /// The paragraphs a selection touches, or `None` with nothing selected.
    fn selected(&self) -> Option<std::ops::RangeInclusive<usize>> {
        self.selection.clone().filter(|r| !r.is_empty()).map(|r| pdfcraft_engine::paragraph_align::selected(self.text, r))
    }

    /// The alignment the buttons show: the first selected paragraph's, or the box's.
    fn shown(&self, look: &AddedText) -> TextAlign {
        self.selected().map_or(look.align, |which| look.paragraph_align(*which.start()))
    }

    /// `align` picked: for the selected paragraphs, or the whole box (dropping theirs).
    fn choose(&self, look: &mut AddedText, align: TextAlign) {
        if !self.tidy && self.selected().is_some() {
            return;
        }
        match self.selected() {
            Some(which) => {
                let count = pdfcraft_engine::paragraph_align::count(self.text);
                look.aligns = pdfcraft_engine::paragraph_align::set(&look.aligns, count, which, align, self.tidy.then_some(look.align));
            }
            None => {
                look.align = align;
                look.aligns.clear();
            }
        }
    }
}

/// What the panel changed: the style (in place) and a font picked from the list.
#[derive(Default)]
pub(crate) struct Outcome {
    pub changed: bool,
    pub size_committed: bool,
    pub font: Option<FontChoice>,
    /// Superscript or Subscript was clicked, for the selected paragraphs' text.
    pub script: Option<Script>,
    /// A colour swatch was clicked, even the one already shown (Thai edition).
    pub colour_picked: bool,
}

const LINE_SPACINGS: [f64; 7] = [1.0, 1.15, 1.2, 1.5, 2.0, 2.5, 3.0];
const SCALES: [f64; 8] = [50.0, 75.0, 90.0, 100.0, 110.0, 125.0, 150.0, 200.0];
const CHAR_SPACINGS: [f64; 8] = [-1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 3.0, 5.0];
/// A row's height: the tallest control in it (the 30 pt icon buttons). Every control is centred
/// on the row, whatever its own height, so lists, swatches and buttons line up (#3037).
const ROW: f32 = 30.0;
/// The gap between a row's controls.
const GAP: f32 = 6.0;
/// The icon naming a list or field ([`labelled_icon`]).
const LABEL_ICON: f32 = 22.0;
/// The button beside a number field that opens its list of common values: just an arrow, so the
/// field keeps the room its value needs.
const PRESET_LIST: f32 = 22.0;
/// The room a number field keeps for its value ("-5.0 pt"), beside the arrow in its box.
const NUMBER_MIN: f32 = 52.0;
/// The size field's room in the row (the edition's size box: its value and its arrow).
const SIZE_FIELD: f32 = 62.0;
/// The least width the panel's widest rows need, which sets how narrow the Tools pane may be
/// dragged. Size and alignment: the size field and its arrow (in their box), the swatch, the
/// divider, three 30 pt buttons, and the gaps between those six. The spacing rows: two icons and
/// two number fields with their arrows (in a box each), the space between the columns, and four
/// gaps.
pub(crate) const MIN_ROW: f32 = {
    let size_row = (SIZE_FIELD + PRESET_LIST + 2.0) + 28.0 + 9.0 + 3.0 * 30.0 + 5.0 * GAP;
    let spacing_row = 2.0 * LABEL_ICON + 2.0 * (NUMBER_MIN + PRESET_LIST + 2.0) + 8.0 + 4.0 * GAP;
    if size_row > spacing_row { size_row } else { spacing_row }
};
/// What can be typed: horizontal scale in percent, character spacing in points.
const SCALE_RANGE: std::ops::RangeInclusive<f64> = 10.0..=400.0;
const CHAR_SPACING_RANGE: std::ops::RangeInclusive<f64> = -5.0..=50.0;

/// A number as the lists show it: no trailing zeros ("1.2", "100", "-0.5").
fn number(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(crate) fn panel(
    ui: &mut egui::Ui,
    t: &Tokens,
    look: &mut AddedText,
    extras: Option<&mut Extras>,
    fonts: Fonts,
    paragraphs: Option<Paragraphs>,
) -> Outcome {
    let before = (look.clone(), extras.as_deref().copied());
    let mut out = Outcome::default();
    // What the pointer rests on, previewed on the text (Thai edition).
    let mut hover = crate::content_ui::LookPreview::default();
    widgets::section_title(ui, tl!("Format text"));

    // The font, across the panel.
    ui.spacing_mut().combo_height = 360.0;
    let width = ui.available_width();
    let font_resp = outlined(ui, |ui| {
        craft_ui::ComboBox::from_id_salt("format-font")
            .selected_text(fonts.shown.as_str())
            .width(width)
            .height(360.0)
            // Typing in the font search must not close the list (Thai edition).
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show_ui(ui, |ui| {
                // Thai edition: one search box on top, for the document's own fonts (listed first)
                // and the standard families, TH Sarabun PSK and the families installed on this
                // machine, as Add text has always offered them.
                let search = ui.make_persistent_id("format-font-search");
                let mut source_choice = None;
                let above = |ui: &mut egui::Ui, want: &str| {
                    let Some(sources) = fonts.sources else { return };
                    let shown: Vec<&(String, String)> = sources
                        .iter()
                        .filter(|(r, face)| !r.starts_with("PCApproved") && (want.is_empty() || face.to_lowercase().contains(want)))
                        .collect();
                    if want.is_empty() && ui.selectable_label(false, tl!("Keep source fonts")).clicked() {
                        source_choice = Some(FontChoice::KeepSource);
                    }
                    if !shown.is_empty() {
                        ui.label(egui::RichText::new(tl!("Fonts in this file")).small().color(ui.visuals().weak_text_color()));
                    }
                    for (resource, face) in shown {
                        if ui.selectable_label(false, face).clicked() {
                            source_choice = Some(FontChoice::Resource(resource.clone()));
                            ui.close();
                        }
                    }
                    ui.separator();
                };
                let family =
                    crate::content_ui::family_list_with(ui, search, fonts.sources.is_none().then_some(look.family), &mut hover.family, above);
                if let Some(choice) = source_choice {
                    out.font = Some(choice);
                }
                if let Some(f) = family {
                    out.font = Some(FontChoice::Family(f));
                }
                if fonts.sources.is_some() {
                    ui.separator();
                    if ui.selectable_label(false, tl!("Load font…")).clicked() {
                        out.font = Some(FontChoice::Load);
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    ui.menu_button(tl!("Installed fonts…"), |ui| {
                        ui.set_min_width(260.0);
                        let reload = ui.button(tl!("Reload")).clicked();
                        match crate::files::installed_font_files(ui.ctx(), reload) {
                            Some(list) if list.is_empty() => {
                                ui.label(tl!("No openly licensed document fonts found. Load a font file."));
                            }
                            Some(list) => {
                                let search_id = ui.scope_id().with("installed-font-search");
                                let mut query = ui.data(|d| d.get_temp::<String>(search_id)).unwrap_or_default();
                                ui.horizontal(|ui| {
                                    let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                                    crate::icons::paint(ui, icon_rect, "search", 13.0, ui.visuals().text_color().gamma_multiply(0.6));
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut query).hint_text(tl!("Search…")).desired_width(ui.available_width() - 8.0),
                                    );
                                    if resp.changed() {
                                        ui.data_mut(|d| d.insert_temp(search_id, query.clone()));
                                    }
                                });
                                ui.separator();

                                let query_lower = query.trim().to_lowercase();
                                let mut families: std::collections::BTreeMap<&str, Vec<&pdfcraft_engine::text_fonts::FontFile>> =
                                    std::collections::BTreeMap::new();
                                for font in &list {
                                    if query_lower.is_empty()
                                        || font.family.to_lowercase().contains(&query_lower)
                                        || font.face.to_lowercase().contains(&query_lower)
                                    {
                                        families.entry(&font.family).or_default().push(font);
                                    }
                                }

                                if families.is_empty() {
                                    ui.label(tl!("No openly licensed document fonts found. Load a font file."));
                                } else {
                                    egui::ScrollArea::vertical().max_height(350.0).show(ui, |ui| {
                                        for (family, faces) in families {
                                            if faces.len() == 1 {
                                                if let Some(&font) = faces.first() {
                                                    let label =
                                                        if font.face != font.family && !font.face.is_empty() { font.face.as_str() } else { family };
                                                    if ui.button(label).clicked() {
                                                        out.font = Some(FontChoice::Installed(font.path.clone()));
                                                        ui.close();
                                                    }
                                                }
                                            } else {
                                                ui.menu_button(family, |ui| {
                                                    ui.set_min_width(180.0);
                                                    for font in faces {
                                                        let label = font
                                                            .face
                                                            .strip_prefix(family)
                                                            .map(str::trim)
                                                            .filter(|s| !s.is_empty())
                                                            .unwrap_or(&font.face);
                                                        if ui.button(label).clicked() {
                                                            out.font = Some(FontChoice::Installed(font.path.clone()));
                                                            ui.close();
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    });
                                }
                            }
                            None => {
                                ui.label(tl!("Finding document fonts…"));
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
            })
            .response
    });
    font_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), tl!("Font")));
    font_resp.tip(format!("{}\n{}", tl!("Font"), tl!("Thai and Arabic use regular fallback fonts when available.")));
    ui.add_space(4.0);

    // Size and colour | bullets, numbering and alignment.
    let mut size_committed = false;
    let mut colour_picked = false;
    row(ui, |ui| {
        // Any size from 1 to 500 pt can be typed, with the usual ones in a list (Thai edition).
        size_committed = crate::content_ui::size_control(ui, &mut look.size, &mut hover.size, fonts.mixed_size);
        let swatch = colour_button(ui, t, look.color);
        egui::Popup::menu(&swatch).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            if let Some(picked) = crate::comments::swatch_grid_hover(ui, Some(look.color), &mut hover.color) {
                // Drops a gray or CMYK ink the new colour isn't the look of (#6440).
                look.set_color(picked);
                colour_picked = true;
                ui.close();
            }
        });
        let (sep, _) = ui.allocate_exact_size(vec2(9.0, 26.0), Sense::hover());
        ui.painter().vline(sep.center().x, sep.y_range().shrink(3.0), Stroke::new(1.0, t.divider));
        planned(ui, "list", tl!("Bulleted list"));
        planned(ui, "list-ordered", tl!("Numbered list"));
        let shown = paragraphs.as_ref().map_or(look.align, |p| p.shown(look));
        let current = match shown {
            TextAlign::Left => "align-left",
            TextAlign::Center => "align-center",
            TextAlign::Right => "align-right",
            TextAlign::Justify => "align-justify",
        };
        let supported = paragraphs.as_ref().is_none_or(|p| p.tidy || p.selected().is_none());
        let align = ui
            .add_enabled_ui(supported, |ui| icons::button(ui, current, 30.0, false, tl!("Alignment")))
            .inner
            .on_disabled_hover_text(format!("{} — {}", tl!("Alignment"), tl!("Planned")));
        let align = flyout(ui, t, align);
        egui::Popup::menu(&align).show(|ui| {
            ui.horizontal(|ui| {
                for (a, icon, tip) in [
                    (TextAlign::Left, "align-left", tl!("Align left")),
                    (TextAlign::Center, "align-center", tl!("Centre")),
                    (TextAlign::Right, "align-right", tl!("Align right")),
                    (TextAlign::Justify, "align-justify", tl!("Justify")),
                ] {
                    if icons::button(ui, icon, 30.0, shown == a, tip).clicked() {
                        match &paragraphs {
                            Some(p) => p.choose(look, a),
                            None => {
                                look.align = a;
                                look.aligns.clear();
                            }
                        }
                        ui.close();
                    }
                }
            });
        });
    });

    // Bold, italic, underline, superscript, subscript.
    let mut extras = extras;
    row(ui, |ui| {
        let bold = icons::button(ui, "bold", 30.0, look.bold, tl!("Bold"));
        if bold.clicked() {
            look.bold = !look.bold;
        }
        hover.bold = toggle_preview(ui, &bold, look.bold);
        let italic = icons::button(ui, "italic", 30.0, look.italic, tl!("Italic"));
        if italic.clicked() {
            look.italic = !look.italic;
        }
        hover.italic = toggle_preview(ui, &italic, look.italic);
        match extras.as_deref_mut() {
            Some(e) => {
                let underline = icons::button(ui, "underline", 30.0, e.underline, tl!("Underline"));
                if underline.clicked() {
                    e.underline = !e.underline;
                }
                hover.underline = toggle_preview(ui, &underline, e.underline);
            }
            None => {
                let underline = icons::button(ui, "underline", 30.0, look.underline, tl!("Underline"));
                if underline.clicked() {
                    look.underline = !look.underline;
                }
                hover.underline = toggle_preview(ui, &underline, look.underline);
            }
        }
        if let Some(p) = &paragraphs {
            for (script, icon, tip) in [(Script::Super, "superscript", tl!("Superscript")), (Script::Sub, "subscript", tl!("Subscript"))] {
                let on = pdfcraft_engine::text_script::is_in(p.text, p.selection.as_ref(), script);
                if icons::button(ui, icon, 30.0, on, tip).clicked() {
                    out.script = Some(script);
                }
            }
        } else {
            planned(ui, "superscript", tl!("Superscript"));
            planned(ui, "subscript", tl!("Subscript"));
        }
    });

    // Line and paragraph spacing; horizontal scale and character spacing: two columns, each an
    // icon and its control, `half` wide.
    // From the font list's width, not what is left now: a row above that overflows (a long
    // translation of "Mixed" in the size field) widens the panel's layout.
    let half = ((width - 2.0 * LABEL_ICON - 8.0 - 3.0 * GAP) / 2.0).max(40.0);
    let editing = extras.is_some();
    let mut fallback = Extras::default();
    let e = extras.as_deref_mut().unwrap_or(&mut fallback);
    // Added text has line spacing of its own too (Thai edition).
    {
        row(ui, |ui| {
            let label = labelled_icon(ui, t, "arrow-up-down", tl!("Line spacing"));
            let own = tl!("Auto").to_string();
            let line_spacing = if editing { &mut e.line_spacing } else { &mut look.line_spacing };
            let shown = if *line_spacing == 0.0 { own.clone() } else { number(*line_spacing) };
            outlined(ui, |ui| {
                // The column is `half` wide in every language: a longer "Auto" is cut short (the
                // menu and the tooltip have it whole).
                ui.set_max_width(half);
                let spacing_resp = craft_ui::ComboBox::from_id_salt("format-line-spacing")
                    .selected_text(centred(ui, &shown))
                    .width(half)
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(line_spacing, 0.0, own.as_str());
                        for v in LINE_SPACINGS {
                            ui.selectable_value(line_spacing, v, number(v));
                        }
                    })
                    .response
                    .labelled_by(label.id);
                spacing_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), tl!("Line spacing")));
                spacing_resp.tip(&shown);
            });
            ui.add_space(8.0);
            ui.add_enabled_ui(false, |ui| {
                let label = labelled_icon(ui, t, "pilcrow", tl!("Paragraph spacing"));
                let tip = format!("{} — {}", tl!("Paragraph spacing"), tl!("Planned"));
                let _ = outlined(ui, |ui| {
                    craft_ui::ComboBox::from_id_salt("format-paragraph-spacing")
                        .selected_text(centred(ui, "0"))
                        .width(half)
                        .show_ui(ui, |_| {})
                        .response
                        .labelled_by(label.id)
                        .disabled_tip(tip)
                });
            });
        });
    }
    // Which way added text runs (Thai edition).
    if fonts.directions {
        row(ui, |ui| {
            // Named in words, not only an icon: the owner couldn't find it by its icon. Where the
            // words would squeeze the list (a long translation in a narrow pane), the icon alone.
            labelled_icon(ui, t, "type", tl!("Text direction"));
            let caption = egui::RichText::new(tl!("Text direction")).color(t.text_muted);
            let words = ui
                .fonts_mut(|f| f.layout_no_wrap(tl!("Text direction").to_owned(), egui::TextStyle::Body.resolve(ui.style()), t.text_muted).size().x);
            if ui.available_width() - words - GAP >= 100.0 {
                ui.label(caption);
            }
            let width = ui.available_width().max(0.0);
            use pdfcraft_engine::TextDirection;
            let name = |d: TextDirection| match d {
                TextDirection::Horizontal => tl!("Horizontal"),
                TextDirection::Stacked => tl!("Vertical, letters upright"),
                TextDirection::Rotated => tl!("Vertical, turned 90°"),
            };
            let resp = outlined(ui, |ui| {
                // Kept to the room left in the row: a long name is cut short (the menu has it whole).
                ui.set_max_width(width);
                craft_ui::ComboBox::from_id_salt("format-text-direction")
                    .selected_text(centred(ui, name(look.direction)))
                    .width(width)
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    .show_ui(ui, |ui| {
                        for d in [TextDirection::Horizontal, TextDirection::Stacked, TextDirection::Rotated] {
                            ui.selectable_value(&mut look.direction, d, name(d));
                        }
                    })
                    .response
            });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), tl!("Text direction")));
        });
    }
    // Any value the engine takes can be typed; the lists hold the common ones. Added text has
    // character spacing of its own (#4245), but no scale yet.
    row(ui, |ui| {
        ui.add_enabled_ui(editing, |ui| {
            let label = labelled_icon(ui, t, "move-horizontal", tl!("Horizontal scale"));
            let scale = Number { value: &mut e.scale, range: SCALE_RANGE, speed: 1.0, suffix: " %" };
            number_with_presets(ui, scale, &SCALES, half, (label.id, tl!("Horizontal scale")));
        });
        ui.add_space(8.0);
        let label = labelled_icon(ui, t, "arrow-left-right", tl!("Character spacing"));
        let value = if editing { &mut e.char_spacing } else { &mut look.char_spacing };
        let spacing = Number { value, range: CHAR_SPACING_RANGE, speed: 0.1, suffix: " pt" };
        number_with_presets(ui, spacing, &CHAR_SPACINGS, half, (label.id, tl!("Character spacing")));
    });

    out.size_committed = size_committed;
    out.colour_picked = colour_picked;
    out.changed = out.changed || size_committed || (look.clone(), extras.as_deref().copied()) != before;
    crate::content_ui::note_hover(ui.ctx(), Some(hover));
    ui.add_space(6.0);
    ui.separator();
    out
}

/// What resting the pointer on a style button (now `on`) previews: the style turned the other way.
/// Not right after a click on it, which already shows the new style: until the pointer leaves.
fn toggle_preview(ui: &egui::Ui, resp: &egui::Response, on: bool) -> Option<bool> {
    let id = resp.id.with("clicked-here");
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(id, true));
    } else if !resp.hovered() {
        ui.data_mut(|d| d.remove::<bool>(id));
    }
    let clicked_here = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    (resp.hovered() && !clicked_here).then_some(!on)
}

/// Runs `add` with the panel's fields outlined and filled, so a list or number field reads as one
/// beside the plain icon buttons around it.
fn outlined<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let t = Tokens::get(ui.ctx());
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = t.field;
        w.inactive.bg_stroke = Stroke::new(1.0, t.border);
        w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
        // A list's text, pad and arrow touch: its gaps would widen it past the column it is given.
        ui.spacing_mut().icon_spacing = 0.0;
        add(ui)
    })
    .inner
}

/// One row of the panel, [`ROW`] high, its controls [`GAP`] apart and centred on it.
fn row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let size = vec2(ui.available_width(), ROW);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        // A list lays itself out from the default control height and grows downward, which leaves
        // it below the row's middle; at a list's own height it is centred like the rest.
        let list = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
        ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(list);
        add(ui)
    })
    .inner
}

/// A control PdfCraft doesn't have yet: its icon, disabled, saying so when hovered.
fn planned(ui: &mut egui::Ui, icon: &str, label: &str) {
    let _ = ui.add_enabled_ui(false, |ui| icons::button(ui, icon, 30.0, false, label)).inner.disabled_tip(format!("{label} — {}", tl!("Planned")));
}

/// The small corner mark Acrobat puts on a button that opens a menu.
fn flyout(ui: &egui::Ui, t: &Tokens, resp: egui::Response) -> egui::Response {
    let r = resp.rect;
    let tri = vec![r.right_bottom() + vec2(-3.0, -3.0), r.right_bottom() + vec2(-8.0, -3.0), r.right_bottom() + vec2(-3.0, -8.0)];
    ui.painter().add(egui::Shape::convex_polygon(tri, t.text_faint, Stroke::NONE));
    resp
}

/// An icon naming the list beside it, for screen readers too. Returns it, for the control it
/// labels.
fn labelled_icon(ui: &mut egui::Ui, t: &Tokens, icon: &str, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(LABEL_ICON, 26.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), label));
    let tint = if ui.is_enabled() { t.icon } else { t.text_faint };
    icons::paint(ui, Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), icon, 17.0, tint);
    resp.tip(label)
}

/// A value typed in a number field: what it edits, what may be typed, and how it reads.
struct Number<'a> {
    value: &'a mut f64,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
    suffix: &'static str,
}

/// A number field with an arrow beside it that lists common values, in `width` all told: type any
/// value in its range, or pick one. Both are named by `label`: the icon before them and its text.
/// A combo box's selected text, centred in its button. The button lays out the text and then its
/// arrow, and every growing atom takes an equal share of the room left: a pad as wide as the arrow
/// before the text balances the arrow after it, and the text centres itself in its own share. Text
/// longer than the list is wide (a translated "Auto") is cut short rather than widening it.
fn centred<'a>(ui: &egui::Ui, text: &str) -> egui::Atoms<'a> {
    use egui::AtomExt as _;
    let pad = egui::Atom::custom(egui::IdSalt::new("combo-pad"), vec2(ui.spacing().icon_width, 0.0)).atom_grow(true);
    let text = egui::Atom::from(text.to_string()).atom_grow(true).atom_shrink(true).atom_align(egui::Align2::CENTER_CENTER);
    egui::Atoms::new((pad, text))
}

/// The arrow that lists a field's common values, centred on the 26-point field beside it.
fn preset_arrow(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (slot, _) = ui.allocate_exact_size(vec2(PRESET_LIST, 26.0), Sense::hover());
    let at = Rect::from_center_size(slot.center(), vec2(PRESET_LIST, PRESET_LIST));
    ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| icons::button(ui, "chevron-down", PRESET_LIST, false, label)).inner
}

/// One box round a value and its arrow, like a list's, so the pair reads as a single control.
fn boxed(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get(ui.ctx());
    let edge = if ui.is_enabled() { t.border } else { t.border.gamma_multiply(0.5) };
    egui::Frame::new().fill(t.field).stroke(Stroke::new(1.0, edge)).corner_radius(CornerRadius::same(4)).show(ui, |ui| {
        let w = &mut ui.visuals_mut().widgets;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.bg_stroke = Stroke::NONE;
        }
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding.x = 2.0;
        add(ui);
    });
}

fn number_with_presets(ui: &mut egui::Ui, n: Number<'_>, presets: &[f64], width: f32, label: (egui::Id, &str)) {
    boxed(ui, |ui| {
        let field = egui::DragValue::new(&mut *n.value).range(n.range).speed(n.speed).max_decimals(2).suffix(n.suffix);
        let drag_resp = ui.add_sized([(width - PRESET_LIST - 2.0).max(30.0), 26.0], field).labelled_by(label.0);
        drag_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SpinButton, ui.is_enabled(), label.1));
        let list = preset_arrow(ui, label.1);
        egui::Popup::menu(&list).show(|ui| {
            for &v in presets {
                if ui.selectable_label(*n.value == v, number(v)).clicked() {
                    *n.value = v;
                    ui.close();
                }
            }
        });
    });
}

/// The text colour as a square swatch that opens the palette.
fn colour_button(ui: &mut egui::Ui, t: &Tokens, color: [f64; 3]) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Colour")));
    let [r, g, b] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let fill = rect.shrink(4.0);
    ui.painter().rect_filled(fill, CornerRadius::same(2), Color32::from_rgb(r, g, b));
    let stroke = if resp.hovered() { t.text } else { t.border };
    ui.painter().rect_stroke(fill, CornerRadius::same(2), Stroke::new(1.0, stroke), egui::StrokeKind::Outside);
    crate::theme::hand(resp).tip(tl!("Colour"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn numbers_read_like_the_lists() {
        assert_eq!(super::number(1.2), "1.2");
        assert_eq!(super::number(100.0), "100");
        assert_eq!(super::number(-0.5), "-0.5");
        assert_eq!(super::number(0.0), "0");
        assert_eq!(super::number(1.15), "1.15");
    }
}
