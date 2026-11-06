//! Guides dragged out of the rulers (Thai edition), as in Photoshop: a press on the top ruler
//! pulls a level guide down onto a page, one on the left ruler an upright guide across. While it
//! is dragged it shows where it is from the page's top left corner in the rulers' units, and it
//! is drawn to the page's edges and centre and to the edges and centres of what is on the page,
//! held there until pulled clearly away (Alt drags it freely). A guide on a page can be dragged
//! again, and dragged back onto a ruler (or off the page) to remove it. Boxes moved or resized
//! snap to the guides. Guides are drawn on screen only, never saved or printed, and show with the
//! rulers.
//!
//! The rulers and each guide take the pointer in an area of their own over the pages, so a press
//! on them is never a page's: the pages see another layer on top there.

use egui::{Color32, CursorIcon, Order, Pos2, Rect, Sense, Stroke};

use super::{DocView, PageXform, RULER};

/// The guides' colour: Photoshop's cyan.
const GUIDE_COLOUR: Color32 = Color32::from_rgb(0x22, 0xC8, 0xE0);

/// A guide's colour while it sits on something it was drawn to.
const HELD_COLOUR: Color32 = Color32::from_rgb(0xFF, 0x2E, 0xC8);

/// How wide a guide is to grab (screen points).
const GRAB: f32 = 6.0;

/// At most this many guides on a document: a cap on what a frame draws.
const MAX_GUIDES: usize = 400;

/// A guide on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RulerGuide {
    pub page: usize,
    /// Upright (from the left ruler), or level (from the top one).
    pub upright: bool,
    /// Where it is across the page as shown: a fraction of its width (upright) or height (level),
    /// so it stays put on the page whatever the zoom.
    pub at: f32,
}

impl RulerGuide {
    /// Where it is on screen over the page shown at `page`: x when upright, y when level.
    fn screen(&self, page: Rect) -> f32 {
        if self.upright { page.left() + self.at * page.width() } else { page.top() + self.at * page.height() }
    }
}

/// A guide being dragged: a new one out of a ruler, or the guide `moving` (its index).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GuideDrag {
    pub upright: bool,
    pub moving: Option<usize>,
}

/// The ruler guides of view `view` on `page` (screen rect `rect`), for boxes to snap to: the
/// upright ones' x and the level ones' y.
pub(crate) fn snap_lines(view: &DocView, page: usize, rect: Rect) -> (Vec<f32>, Vec<f32>) {
    if !view.guides_shown {
        return (Vec::new(), Vec::new());
    }
    let mut out = (Vec::new(), Vec::new());
    for (k, g) in view.ruler_guides.iter().enumerate() {
        // The guide being dragged is not there yet.
        if g.page != page || view.guide_drag.is_some_and(|d| d.moving == Some(k)) {
            continue;
        }
        let at = g.screen(rect);
        if at.is_finite() {
            if g.upright { out.0.push(at) } else { out.1.push(at) }
        }
    }
    out
}

/// Where the pointer is, and whether Escape was pressed: the guides read egui's pointer for the
/// drags they own, which began on their own areas, not on the pages.
fn pointer(ui: &egui::Ui) -> (Option<Pos2>, bool, bool) {
    ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down(), i.key_pressed(egui::Key::Escape)))
}

/// The page shown under `p` (or, beside the pages, the nearest along the guide's way), among
/// `pages`.
fn page_at(pages: &[(usize, PageXform)], p: Pos2) -> Option<(usize, Rect)> {
    if let Some((i, xf)) = pages.iter().find(|(_, xf)| xf.rect.contains(p)) {
        return Some((*i, xf.rect));
    }
    None
}

/// The lines a guide on `page` (screen) is drawn to: the page's edges and centre, and the edges
/// and centres of `boxes`.
fn lines(page: Rect, boxes: &[Rect], upright: bool) -> Vec<f32> {
    let mut out = if upright { vec![page.left(), page.center().x, page.right()] } else { vec![page.top(), page.center().y, page.bottom()] };
    for b in boxes {
        if upright {
            out.extend([b.left(), b.center().x, b.right()]);
        } else {
            out.extend([b.top(), b.center().y, b.bottom()]);
        }
    }
    out.retain(|v| v.is_finite());
    out
}

/// The text beside the pointer: the guide's distance from the page's top left corner.
fn readout(upright: bool, points: f32, units: crate::units::Unit) -> String {
    let v = f64::from(points) * units.per_point();
    format!("{}: {v:.prec$} {}", if upright { "X" } else { "Y" }, units.label(), prec = units.decimals().min(3))
}

/// What the guides need of the document area this frame.
pub(crate) struct GuideFrame<'a> {
    /// The document area, rulers included.
    pub area: Rect,
    pub units: crate::units::Unit,
    /// The boxes on page `n` (screen), for a guide to be drawn to.
    pub boxes: &'a dyn Fn(usize) -> Vec<Rect>,
    /// A dialog is open over the window: the guides show, but nothing takes hold of them.
    pub modal: bool,
}

/// The guides of `view`: drawn over the pages, dragged out of the rulers, moved and removed. Call
/// after the pages, with the rulers showing.
pub(crate) fn guides(ui: &egui::Ui, view: &mut DocView, frame: &GuideFrame<'_>) {
    let area = frame.area;
    let top = Rect::from_min_max(Pos2::new(area.left() + RULER, area.top()), Pos2::new(area.right(), area.top() + RULER));
    let left = Rect::from_min_max(Pos2::new(area.left(), area.top() + RULER), Pos2::new(area.left() + RULER, area.bottom()));
    let inside = Rect::from_min_max(area.min + egui::vec2(RULER, RULER), area.max);
    let pages = view.screen_xforms.clone();
    let ctx = ui.ctx().clone();
    let painter = ui.painter().with_clip_rect(inside);

    if frame.modal {
        view.guide_drag = None;
    }
    // The rulers take a press that drags a new guide out.
    for (strip, upright, name) in [(top, false, "top"), (left, true, "left")] {
        if !strip.is_positive() || frame.modal {
            continue;
        }
        let resp = grab_area(
            &ctx,
            egui::Id::unique(("ruler-strip", view.id.0, name)),
            strip,
            if upright { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical },
        );
        if resp.drag_started() && view.guide_drag.is_none() && view.ruler_guides.len() < MAX_GUIDES {
            view.guide_drag = Some(GuideDrag { upright, moving: None });
        }
    }

    // The guides on the pages shown, each with a strip to take hold of it by.
    let dragged = view.guide_drag.and_then(|d| d.moving);
    let mut take = None;
    for (k, g) in view.ruler_guides.iter().enumerate() {
        let Some((_, xf)) = pages.iter().find(|(i, _)| *i == g.page) else { continue };
        if Some(k) == dragged {
            continue;
        }
        let at = g.screen(xf.rect);
        if !at.is_finite() {
            continue;
        }
        let (a, b) = if g.upright {
            (Pos2::new(at, xf.rect.top()), Pos2::new(at, xf.rect.bottom()))
        } else {
            (Pos2::new(xf.rect.left(), at), Pos2::new(xf.rect.right(), at))
        };
        painter.line_segment([a, b], Stroke::new(1.0, GUIDE_COLOUR));
        let strip = Rect::from_two_pos(a, b).expand(GRAB / 2.0).intersect(inside);
        if strip.is_positive() && view.guide_drag.is_none() && !frame.modal {
            let icon = if g.upright { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical };
            if grab_area(&ctx, egui::Id::unique(("ruler-guide", view.id.0, k)), strip, icon).drag_started() {
                take = Some(GuideDrag { upright: g.upright, moving: Some(k) });
            }
        }
    }
    if take.is_some() {
        view.guide_drag = take;
    }

    let Some(drag) = view.guide_drag else { return };
    let (at, down, escape) = pointer(ui);
    let Some(p) = at else {
        // Let go outside the window: the drag ends where it began.
        if !down || escape {
            view.guide_drag = None;
        }
        return;
    };
    let on_ruler = top.contains(p) || left.contains(p) || !area.contains(p);
    let target = page_at(&pages, p).or_else(|| {
        // A guide being moved stays on its page while the pointer is between pages.
        drag.moving.and_then(|k| view.ruler_guides.get(k)).and_then(|g| pages.iter().find(|(i, _)| *i == g.page)).map(|(i, xf)| (*i, xf.rect))
    });
    // Where it goes: snapped (and held) on the page's lines.
    let placed = target.filter(|_| !on_ruler).map(|(page, rect)| {
        let along = if drag.upright { p.x } else { p.y };
        let (to, held) = crate::content_ui::snap_line(ui, along, &lines(rect, &(frame.boxes)(page), drag.upright));
        (page, rect, to, held.is_some())
    });
    if escape {
        view.guide_drag = None;
        return;
    }
    if !down {
        // Let go: on a page it stays (or moves) there; on a ruler or off the pages it goes.
        view.guide_drag = None;
        match (placed, drag.moving) {
            (Some((page, rect, to, _)), moving) => {
                let span = if drag.upright { rect.width() } else { rect.height() };
                let from = if drag.upright { rect.left() } else { rect.top() };
                let fraction = (to - from) / span;
                if span > 0.0 && fraction.is_finite() {
                    let g = RulerGuide { page, upright: drag.upright, at: fraction };
                    match moving.and_then(|k| view.ruler_guides.get_mut(k)) {
                        Some(slot) => *slot = g,
                        None => view.ruler_guides.push(g),
                    }
                }
            }
            (None, Some(k)) if k < view.ruler_guides.len() => {
                view.ruler_guides.remove(k);
            }
            (None, _) => {}
        }
        ctx.request_repaint();
        return;
    }
    // The guide under way: across the whole document area, as Photoshop draws it, with where it
    // is beside the pointer.
    let fg = ctx.layer_painter(egui::LayerId::new(Order::Foreground, egui::Id::unique(("ruler-guide-drag", view.id.0)))).with_clip_rect(inside);
    // Over a ruler or off the pages nothing shows: letting go there removes it.
    if let Some((_, rect, to, held)) = placed {
        let colour = if held { HELD_COLOUR } else { GUIDE_COLOUR };
        let (a, b) = if drag.upright {
            (Pos2::new(to, inside.top()), Pos2::new(to, inside.bottom()))
        } else {
            (Pos2::new(inside.left(), to), Pos2::new(inside.right(), to))
        };
        fg.line_segment([a, b], Stroke::new(1.0, colour));
        let zoom = pages.iter().find(|(_, xf)| xf.rect == rect).map_or(1.0, |(_, xf)| xf.zoom());
        let from = if drag.upright { rect.left() } else { rect.top() };
        let points = (to - from) / zoom.max(1e-3);
        if points.is_finite() {
            let text = readout(drag.upright, points, frame.units);
            let galley = fg.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
            let at = if drag.upright { Pos2::new(to + 12.0, p.y + 12.0) } else { Pos2::new(p.x + 12.0, to + 12.0) };
            let r = Rect::from_min_size(at, galley.size() + egui::vec2(12.0, 8.0));
            fg.rect_filled(r, egui::CornerRadius::same(3), Color32::from_rgba_unmultiplied(40, 40, 40, 230));
            fg.galley(r.min + egui::vec2(6.0, 4.0), galley, Color32::WHITE);
        }
    }
    ctx.set_cursor_icon(if drag.upright { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical });
    ctx.request_repaint();
}

/// An area of its own over the pages at `rect` that takes a drag, with `icon` over it.
fn grab_area(ctx: &egui::Context, id: egui::Id, rect: Rect, icon: CursorIcon) -> egui::Response {
    egui::Area::new(id)
        .order(Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .interactable(true)
        .show(ctx, |ui| {
            let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::drag());
            resp.on_hover_cursor(icon)
        })
        .inner
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2, vec2};

    use super::{RulerGuide, lines, readout};

    #[test]
    fn a_guide_keeps_its_place_on_the_page_at_any_zoom() {
        let g = RulerGuide { page: 0, upright: true, at: 0.25 };
        assert_eq!(g.screen(Rect::from_min_size(pos2(100.0, 0.0), vec2(400.0, 600.0))), 200.0);
        assert_eq!(g.screen(Rect::from_min_size(pos2(100.0, 0.0), vec2(800.0, 1200.0))), 300.0);
        let level = RulerGuide { page: 0, upright: false, at: 0.5 };
        assert_eq!(level.screen(Rect::from_min_size(pos2(0.0, 50.0), vec2(400.0, 600.0))), 350.0);
    }

    #[test]
    fn a_guide_is_drawn_to_the_page_centre_and_the_boxes_edges() {
        let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        let b = Rect::from_min_size(pos2(30.0, 40.0), vec2(100.0, 20.0));
        assert_eq!(lines(page, &[b], true), [0.0, 200.0, 400.0, 30.0, 80.0, 130.0]);
        assert_eq!(lines(page, &[b], false), [0.0, 300.0, 600.0, 40.0, 50.0, 60.0]);
    }

    #[test]
    fn the_readout_is_in_the_rulers_units() {
        assert_eq!(readout(true, 72.0, crate::units::Unit::Inches), "X: 1.00 in");
        assert!(readout(false, 72.0, crate::units::Unit::Millimetres).starts_with("Y: 25.4"));
    }
}
