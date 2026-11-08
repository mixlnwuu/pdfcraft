//! Edit a PDF ▸ Add content: the Text tool (click on the page and type), Image (pick a file;
//! it lands in the middle of the page), and editing what was added: select, move, resize
//! (images keep their proportions), double-click text to retype it, format it from the panel,
//! Delete to remove it. Each change is one undoable engine edit.

#[cfg(not(target_arch = "wasm32"))]
use crate::file_filter::AddFilterAnyCase as _;
use crate::widgets::Tip as _;
use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{Added, AddedContent, AddedText, Arrange, Edit, FontFamily, TextAlign};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::page_router::{Hit, Owner};
use crate::theme::Tokens;
use crate::widgets;

/// The colour of the warning under text the standard fonts can't draw.
pub(crate) const UNDRAWABLE: Color32 = Color32::from_rgb(0xD7, 0x37, 0x3F);

/// The first character of a draft that can't be drawn, as the engine decides it (it refuses
/// such text, #125): outside the standard fonts and not shaped with the Arabic face.
pub(crate) fn undrawable(d: &TextDraft, memo: &UndrawableMemo) -> Option<char> {
    // Checked as `finish` will commit it: without trailing whitespace.
    let t = AddedText { text: d.text.trim_end().to_string(), rect: d.rect, ..d.style.clone() };
    if let Some((key, c)) = memo.borrow().as_ref()
        && *key == t
    {
        return *c;
    }
    let c = pdfcraft_engine::first_undrawable(&t);
    *memo.borrow_mut() = Some((t, c));
    c
}

/// The last draft [`undrawable`] checked and its answer: the check lays the text out (and shapes
/// Arabic), too slow to repeat every frame for long text, so it reruns only when the text or
/// anything that changes its layout does.
pub(crate) type UndrawableMemo = std::cell::RefCell<Option<(AddedText, Option<char>)>>;

/// The warning shown for text with `c` in it.
pub(crate) fn undrawable_message(c: char) -> String {
    crate::i18n::fmt(
        tl!("The PDF text fonts can't draw “{c}” ({code}). Choose supported text."),
        &[("c", &c.to_string()), ("code", &format!("U+{:04X}", u32::from(c)))],
    )
}

/// The warning after an edit that saved text its font can't draw: PdfCraft drew `?` for it (a form
/// field, which other viewers redraw from its value, or a comment, where the `?` shows everywhere).
pub(crate) fn undrawn_message(n: &pdfcraft_engine::TextNotice) -> String {
    let template = if n.redrawn_elsewhere {
        tl!(
            "This field's font can't draw “{c}” ({code}), so PdfCraft shows ? in its place. The value is saved, and other PDF viewers redraw the field from it."
        )
    } else {
        tl!("The fonts PdfCraft writes with can't draw “{c}” ({code}), so it shows as ? in every PDF viewer. The text is saved.")
    };
    crate::i18n::fmt(template, &[("c", &n.ch.to_string()), ("code", &format!("U+{:04X}", u32::from(n.ch)))])
}

const SELECT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// The text being typed or retyped in place.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDraft {
    pub page: usize,
    /// `None` for new text; the item index for retyping.
    pub index: Option<usize>,
    /// Display space.
    pub rect: [f64; 4],
    pub text: String,
    /// The box's formatting; its `text` and `rect` are what the page has (empty for new text), and
    /// its `aligns` follow `text` as it is typed.
    pub style: AddedText,
    pub focus: bool,
    /// The box hugs the text: new text, and retyped text whose box didn't wrap it. A box made
    /// narrow enough to wrap keeps its width.
    pub auto: bool,
    /// The selected bytes of `text` (empty: the caret), for aligning the lines it touches.
    pub selection: Option<std::ops::Range<usize>>,
    /// The text the box's own Undo history starts from ([`crate::edit_text_ui::forget_typing_before`]).
    pub typing_from: Option<String>,
}

impl TextDraft {
    /// Take the shared text box for this box ([`crate::edit_text_ui::forget_typing_before`]): its
    /// own history, or none once the page has its text.
    pub(crate) fn claim_typing(&mut self, ctx: &egui::Context, doc: pdfcraft_engine::DocId) {
        crate::edit_text_ui::forget_typing_before(ctx, input_id(), doc, &mut self.typing_from, &self.style.text);
    }

    /// Typing the page doesn't have yet: Undo takes it back in the box before touching the document.
    pub(crate) fn has_unapplied_typing(&self) -> bool {
        self.text != self.style.text
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// Which corner: (left, top) in screen terms.
    Corner(bool, bool),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentView {
    /// The selected added item: (page, index among the page's items).
    pub selected: Option<(usize, usize)>,
    pub draft: Option<TextDraft>,
    /// Select the item an add creates (the page's item count before it).
    pub select_added: Option<(usize, usize)>,
    /// Keep the selection on this item (page, stream) once Arrange has changed its index.
    pub follow: Option<(usize, pdfcraft_cos::ObjRef)>,
    /// The item (page, index) the page's right-click menu is for, set by a right-click on it.
    pub menu: Option<(usize, usize)>,
    /// A right-click on a box open for typing, unchanged (Thai edition): the box closed and the
    /// page's menu is to open here, with its items (Split into lines, Arrange).
    pub menu_at: Option<Pos2>,
    grab: Option<(usize, Grab, Pos2)>,
    undrawable: UndrawableMemo,
    layout: Option<(AddedText, f32, f64, [f64; 4])>,
}

impl ContentView {
    /// Forget an in-progress pointer grab for content editing.
    pub(crate) fn cancel_pointer_gesture(&mut self) {
        self.grab = None;
    }

    pub(crate) fn finish_draft(&mut self, added: &[Added], info: &DocInfo) -> Option<Edit> {
        self.draft.take().and_then(|d| finish(self, d, added, info))
    }
    /// The draft's first character the standard fonts can't draw. Such a draft is kept open, and
    /// counts as unsaved work, until it is corrected or discarded.
    pub(crate) fn blocked(&self) -> Option<char> {
        self.draft.as_ref().and_then(|d| undrawable(d, &self.undrawable))
    }

    /// Keeps a blocked draft open and focused instead of finishing or replacing it.
    pub(crate) fn hold_blocked(&mut self) -> bool {
        let blocked = self.blocked().is_some();
        if let Some(d) = self.draft.as_mut().filter(|_| blocked) {
            d.focus = true;
        }
        blocked
    }
}

/// While an added item is moved, or a picture resized by a corner, the page without it, rendered
/// once per gesture: the item's own pixels follow the pointer over it instead of staying put until
/// release (#1488).
#[derive(Default)]
pub(crate) struct MoveBackground {
    key: Option<(u64, usize, usize)>,
    renderer: Option<pdfcraft_render::RenderPool>,
    texture: Option<egui::TextureHandle>,
    /// The item alone on a transparent page: the pixels that follow the pointer. The page's own
    /// raster has whatever the item covers in its box too, which must stay where it is (#4464).
    item_renderer: Option<pdfcraft_render::RenderPool>,
    item: Option<egui::TextureHandle>,
    tag: Option<u64>,
    /// The item's display-space box when the gesture started.
    from: Option<[f64; 4]>,
    /// The item is a picture: a corner stretches its pixels, and it keeps its whole applied box.
    picture: bool,
    /// The applied move's box. The page raster on screen still has the item where it was until
    /// its replacement arrives, so the preview stays up until then.
    settling: Option<[f64; 4]>,
}

impl MoveBackground {
    pub fn pending(&self) -> bool {
        self.renderer.is_some() && self.item_renderer.is_some() && (self.texture.is_none() || self.item.is_none())
    }

    /// Both rasters are in: the page without the item, and the item alone.
    fn ready(&self) -> bool {
        self.texture.is_some() && self.item.is_some()
    }

    pub fn prepare(&mut self, ctx: &egui::Context, doc: &pdfcraft_engine::Document, cv: &ContentView, scale: f32) {
        let picture = |page, index| on_page(doc.added(), page).into_iter().any(|(i, a)| i == index && matches!(a.content, AddedContent::Image(_)));
        let key = match (cv.grab, cv.selected) {
            (Some((gp, Grab::Move, _)), Some((sp, si))) if gp == sp => Some((doc.edit_generation(), sp, si)),
            // Text resized by a corner rewraps, so only a picture's pixels can follow the corner.
            (Some((gp, Grab::Corner(..), _)), Some((sp, si))) if gp == sp && picture(sp, si) => Some((doc.edit_generation(), sp, si)),
            _ => None,
        };
        if !(key.is_none() && self.settling.is_some()) && self.key != key {
            *self = Self { key, ..Default::default() };
            if let Some((_, page, index)) = key {
                self.renderer = doc.added_content_preview(page, index).ok().map(|p| p.renderer());
                self.item_renderer = doc.added_content_alone(page, index).ok().map(|p| p.renderer());
                self.from = on_page(doc.added(), page).into_iter().find(|(i, _)| *i == index).map(|(_, a)| a.content.rect());
                self.picture = picture(page, index);
            }
        }
        if self.settling.is_some() {
            return;
        }
        let (Some((_, page, _)), Some(renderer), Some(item_renderer)) = (self.key, self.renderer.as_ref(), self.item_renderer.as_ref()) else {
            return;
        };
        let Some(p) = doc.info.pages.get(page) else { return };
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let scale = scale.min(crate::canvas::BASE_SIDE.min(max_side) / p.width.max(p.height).max(1.0));
        let tag = (scale * 1000.0) as u64;
        if self.tag != Some(tag) {
            for r in [renderer, item_renderer] {
                r.set_queue(vec![pdfcraft_render::RenderRequest { page, scale, tag, ..Default::default() }]);
            }
            self.tag = Some(tag);
        }
        if self.receive(ctx, tag) && self.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// Takes in the rasters rendered at `tag`. `false` when either failed: the preview is then
    /// dropped whole, including rasters in from an earlier scale.
    fn receive(&mut self, ctx: &egui::Context, tag: u64) -> bool {
        let (Some(renderer), Some(item_renderer)) = (self.renderer.as_ref(), self.item_renderer.as_ref()) else {
            return false;
        };
        let mut failed = false;
        for (r, slot, name) in
            [(renderer, &mut self.texture, "added-content-move-background"), (item_renderer, &mut self.item, "added-content-move-item")]
        {
            let Some(out) = r.try_recv() else { continue };
            if out.error.is_some() {
                failed = true;
            } else if out.request.tag == tag {
                *slot = Some(ctx.load_texture(
                    name,
                    crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba),
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        if failed {
            // Without both rasters the item stays put while dragged, as before.
            self.renderer = None;
            self.item_renderer = None;
            self.texture = None;
            self.item = None;
        }
        !failed
    }

    /// The move was applied: keep the preview, now at the new generation, until the page is
    /// rendered again. Any other edit ends it.
    pub fn committed(&mut self, edit: &Edit, generation: u64) {
        match edit {
            Edit::UpdateContent { page, index, content } if self.ready() && self.key.is_some_and(|(_, p, i)| (p, i) == (*page, *index)) => {
                self.key = Some((generation, *page, *index));
                self.settling = Some(content.rect());
            }
            _ => *self = Self::default(),
        }
    }

    pub fn page_received(&mut self, page: usize) {
        if self.settling.is_some() && self.key.is_some_and(|(_, p, _)| p == page) {
            *self = Self::default();
        }
    }

    /// The page without the moving item, then the item's own pixels (from its raster alone, so
    /// nothing it covered comes along) at the dragged (to the pages' `pointer`) or applied
    /// position. `ui`: Alt held moves freely, off the smart guides (Thai edition).
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        xf: &PageXform,
        page: usize,
        info: &DocInfo,
        cv: &ContentView,
        pointer: Option<Pos2>,
    ) {
        let (Some((_, kp, _)), Some(background), Some(item), Some(from)) = (self.key, &self.texture, &self.item, self.from) else { return };
        if kp != page {
            return;
        }
        let r = screen_rect(xf, info, page, from);
        let to = if let Some((_, grab, start)) = cv.grab {
            let Some(p) = pointer else { return };
            match grab {
                Grab::Move => dragged(r, grab, snap_to_centre(ui, xf.rect, r, p - start), self.picture),
                Grab::Corner(left, top) => {
                    snap_resize(ui, xf.rect, r, dragged(r, grab, p - start, self.picture), Some(left), Some(top), self.picture)
                }
            }
        } else if let Some(to) = self.settling {
            let to = screen_rect(xf, info, page, to);
            // A moved text box keeps its top edge (its height follows its lines).
            if self.picture { to } else { r.translate(to.left_top() - r.left_top()) }
        } else {
            return;
        };
        let painter = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
        xf.paint_image(&painter, background.id(), 0.0, 0.0, 1.0, 1.0);
        // A pixel around the box, so its edges come along.
        let (r, to) = (r.expand(1.0), to.expand(1.0));
        let mut mesh = egui::Mesh::with_texture(item.id());
        for (a, b) in [
            (r.left_top(), to.left_top()),
            (r.right_top(), to.right_top()),
            (r.right_bottom(), to.right_bottom()),
            (r.left_bottom(), to.left_bottom()),
        ] {
            let (u, v) = xf.screen_to_norm(a);
            mesh.vertices.push(egui::epaint::Vertex { pos: b, uv: Pos2::new(u, v), color: Color32::WHITE });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// Text style for new text (the panel's Format controls set it).
pub fn default_style() -> AddedText {
    AddedText { size: 12.0, ..AddedText::default() }
}

/// Display-space rect → screen.
fn screen_rect(xf: &PageXform, info: &DocInfo, page: usize, r: [f64; 4]) -> Rect {
    let ph = info.pages[page].height as f64;
    xf.view_rect([r[0] as f32, (ph - r[3]) as f32, r[2] as f32, (ph - r[1]) as f32])
}

/// Screen point → display space.
fn to_display(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    [vx as f64, info.pages[page].height as f64 - vy as f64]
}

fn on_page(added: &[Added], page: usize) -> Vec<(usize, &Added)> {
    added.iter().filter(|a| a.page == page).enumerate().collect()
}

pub(crate) fn hits_added(xf: &PageXform, info: &DocInfo, page: usize, added: &[Added], point: Pos2) -> bool {
    on_page(added, page)
        .iter()
        .filter(|(_, a)| matches!(a.content, AddedContent::Text(_)))
        .any(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(point))
}

fn handles(r: Rect) -> [(Pos2, bool, bool); 4] {
    [(r.left_top(), true, true), (r.right_top(), false, true), (r.left_bottom(), true, false), (r.right_bottom(), false, false)]
}

/// How close (screen points) a moved box's edge or centre comes to a guide before it snaps onto
/// it.
const SNAP: f32 = 6.0;

/// Smart guides' colour, as in Photoshop.
const GUIDE: Color32 = Color32::from_rgb(0xFF, 0x2E, 0xC8);

/// The page's centre lines' colour: the green they had before the smart guides, so they stand
/// apart from the lines to other boxes (Thai edition, the owner's choice).
pub(crate) const CENTRE_GUIDE: Color32 = Color32::from_rgb(0x2E, 0xB8, 0x5C);

/// Once a box has snapped to a guide it stays on it until the pointer pulls this far (screen
/// points) away, so it feels held, as Photoshop's guides do (Thai edition).
const RELEASE: f32 = 14.0;

/// How long (seconds) without a snapped move before a hold is forgotten: the next drag starts free.
const HOLD_FORGET: f64 = 0.4;

/// Whether smart guides show and boxes snap to them (View ▸ Smart Guides, Snap to Guides; Thai
/// edition), for this frame.
pub(crate) fn set_guide_options(ctx: &egui::Context, show: bool, snap: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("guide-options"), (show, snap)));
}

/// Whether things snap this frame: Snap to Guides is on and Alt is not held (Thai edition).
pub(crate) fn snapping(ui: &egui::Ui) -> bool {
    guide_options(ui.ctx()).1 && !ui.input(|i| i.modifiers.alt)
}

fn guide_options(ctx: &egui::Context) -> (bool, bool) {
    ctx.data(|d| d.get_temp::<(bool, bool)>(egui::Id::unique("guide-options"))).unwrap_or((true, true))
}

/// The guide a moving box holds on to along each axis: the guide's position, and which of the
/// box's lines (left/centre/right or top/middle/bottom) sits on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Hold {
    pub x: Option<(f32, usize)>,
    pub y: Option<(f32, usize)>,
}

/// What else is on `page` (screen boxes) for smart guides to line a moved box up with, kept for
/// this frame (Thai edition).
pub(crate) fn set_snap_targets(ctx: &egui::Context, page: Rect, boxes: Vec<Rect>) {
    ctx.data_mut(|d| d.insert_temp(snap_id(page), std::sync::Arc::new(boxes)));
}

/// The grid step on screen over `page` (`None`: no grid), for smart guides to snap to too
/// (Thai edition).
pub(crate) fn set_snap_grid(ctx: &egui::Context, page: Rect, step: Option<f32>) {
    ctx.data_mut(|d| d.insert_temp(grid_id(page), step));
}

/// The ruler guides over `page` (screen x of the upright ones, y of the level ones), for boxes to
/// snap to as well (Thai edition).
pub(crate) fn set_snap_lines(ctx: &egui::Context, page: Rect, xs: Vec<f32>, ys: Vec<f32>) {
    ctx.data_mut(|d| d.insert_temp(lines_id(page), std::sync::Arc::new((xs, ys))));
}

fn lines_id(page: Rect) -> egui::Id {
    egui::Id::unique(("snap-lines", page.min.x.to_bits(), page.min.y.to_bits()))
}

/// The ruler guides set for `page` this frame.
fn snap_lines(ctx: &egui::Context, page: Rect) -> std::sync::Arc<(Vec<f32>, Vec<f32>)> {
    ctx.data(|d| d.get_temp(lines_id(page))).unwrap_or_default()
}

/// A ruler guide being dragged to `at` (screen x or y, Thai edition): snapped onto the nearest of
/// `lines` (page edges and centre, boxes' edges and centres) within [`SNAP`], and held there until
/// it is pulled [`RELEASE`] away, as a moved box is. Returns where it goes and the line it sits on.
/// Alt held, or Snap to Guides off, moves it freely.
pub(crate) fn snap_line(ui: &egui::Ui, at: f32, lines: &[f32]) -> (f32, Option<f32>) {
    let ctx = ui.ctx();
    let id = egui::Id::unique("snap-hold-guide");
    if ui.input(|i| i.modifiers.alt) || !guide_options(ctx).1 || !at.is_finite() {
        ctx.data_mut(|g| g.remove::<(Option<(f32, usize)>, f64)>(id));
        return (at, None);
    }
    let now = ui.input(|i| i.time);
    let mut held = ctx.data(|g| g.get_temp::<(Option<(f32, usize)>, f64)>(id)).filter(|(_, t)| now - t <= HOLD_FORGET).and_then(|(h, _)| h);
    let out = snap_axis(&[at], lines.iter().copied(), &mut held);
    ctx.data_mut(|g| g.insert_temp(id, (held, now)));
    match out {
        Some(shift) => (at + shift, held.map(|(g, _)| g)),
        None => (at, None),
    }
}

fn grid_id(page: Rect) -> egui::Id {
    egui::Id::unique(("snap-grid", page.min.x.to_bits(), page.min.y.to_bits()))
}

fn snap_id(page: Rect) -> egui::Id {
    egui::Id::unique(("snap-targets", page.min.x.to_bits(), page.min.y.to_bits()))
}

/// The boxes to line up with on `page`, without those inside `from` (what is being moved).
fn snap_targets(ctx: &egui::Context, page: Rect, from: Rect) -> Vec<Rect> {
    let all: Option<std::sync::Arc<Vec<Rect>>> = ctx.data(|d| d.get_temp(snap_id(page)));
    let inside = from.expand(0.5);
    all.map(|v| v.iter().filter(|b| b.is_positive() && !inside.contains_rect(**b)).copied().collect()).unwrap_or_default()
}

/// Smart guides (Thai edition), as in Photoshop: the move `d` of a box `r` (screen), adjusted so
/// one of its edges or its centre lands on another box's edge or centre, or on the page's centre
/// lines, when it comes within [`SNAP`] of one. Alt held moves freely.
pub(crate) fn snap_to_centre(ui: &egui::Ui, page: Rect, r: Rect, d: egui::Vec2) -> egui::Vec2 {
    let ctx = ui.ctx();
    if ui.input(|i| i.modifiers.alt) || !guide_options(ctx).1 {
        return d;
    }
    let grid = ctx.data(|g| g.get_temp::<Option<f32>>(grid_id(page))).flatten();
    // The hold of this drag, unless the last snapped move was a while ago (another drag).
    let id = egui::Id::unique("snap-hold");
    let now = ui.input(|i| i.time);
    let mut hold = ctx.data(|g| g.get_temp::<(Hold, f64)>(id)).filter(|(_, at)| now - at <= HOLD_FORGET).map(|(h, _)| h).unwrap_or_default();
    let out = snap_delta_held(page, r, d, &snap_targets(ctx, page, r), grid, &snap_lines(ctx, page), &mut hold);
    ctx.data_mut(|g| g.insert_temp(id, (hold, now)));
    out
}

/// The lines a box lines up on along one axis: its two edges and its centre.
fn lines_x(r: Rect) -> [f32; 3] {
    [r.left(), r.center().x, r.right()]
}

fn lines_y(r: Rect) -> [f32; 3] {
    [r.top(), r.center().y, r.bottom()]
}

/// The nearest guide to any of `moving` within [`SNAP`]: how far to shift onto it, the guide,
/// and which of `moving` goes onto it.
fn nearest(moving: &[f32], guides: impl Iterator<Item = f32>) -> Option<(f32, f32, usize)> {
    let mut best: Option<(f32, f32, usize)> = None;
    for g in guides {
        for (k, &m) in moving.iter().enumerate() {
            let shift = g - m;
            if shift.abs() <= SNAP && best.is_none_or(|b| shift.abs() < b.0.abs()) {
                best = Some((shift, g, k));
            }
        }
    }
    best
}

/// Along one axis: a guide within reach snaps (and is held), else the held guide keeps the box
/// while its line stays within [`RELEASE`] of it; past that the hold lets go.
fn snap_axis(moving: &[f32], guides: impl Iterator<Item = f32>, held: &mut Option<(f32, usize)>) -> Option<f32> {
    if let Some((shift, g, k)) = nearest(moving, guides) {
        *held = Some((g, k));
        return Some(shift);
    }
    let shift = held.and_then(|(g, k)| moving.get(k).map(|m| g - m)).filter(|s| s.abs() <= RELEASE);
    if shift.is_none() {
        *held = None;
    }
    shift
}

/// [`snap_to_centre`] without the Alt key, lining up with `others` too, and with the grid lines
/// `grid` apart from the page's top left when the grid shows.
#[cfg(test)]
fn snap_delta(page: Rect, r: Rect, d: egui::Vec2, others: &[Rect], grid: Option<f32>) -> egui::Vec2 {
    snap_delta_held(page, r, d, others, grid, &(Vec::new(), Vec::new()), &mut Hold::default())
}

/// [`snap_delta`], held on to the guides in `hold` (see [`RELEASE`]) and updating it.
fn snap_delta_held(
    page: Rect,
    r: Rect,
    d: egui::Vec2,
    others: &[Rect],
    grid: Option<f32>,
    (rx, ry): &(Vec<f32>, Vec<f32>),
    hold: &mut Hold,
) -> egui::Vec2 {
    let moved = r.translate(d);
    // The grid lines nearest the box's edges.
    let near = |origin: f32, edges: [f32; 2]| -> Vec<f32> {
        grid.filter(|s| s.is_finite() && *s >= 1.0).map_or_else(Vec::new, |s| edges.iter().map(|m| origin + ((m - origin) / s).round() * s).collect())
    };
    let xs = std::iter::once(page.center().x)
        .chain(others.iter().flat_map(|o| lines_x(*o)))
        .chain(near(page.left(), [moved.left(), moved.right()]))
        .chain(rx.iter().copied());
    let ys = std::iter::once(page.center().y)
        .chain(others.iter().flat_map(|o| lines_y(*o)))
        .chain(near(page.top(), [moved.top(), moved.bottom()]))
        .chain(ry.iter().copied());
    let mut out = d;
    if let Some(shift) = snap_axis(&lines_x(moved), xs, &mut hold.x) {
        out.x += shift;
    }
    if let Some(shift) = snap_axis(&lines_y(moved), ys, &mut hold.y) {
        out.y += shift;
    }
    out
}

/// Smart guides while resizing (Thai edition): the box `from` (screen) resized to `to` by its
/// edges `x` and `y` (`Some(true)`: the left or top edge moves, `Some(false)`: the right or
/// bottom one, `None`: that axis doesn't change), with the moving edges snapped onto other boxes'
/// edges and centres, the page's centre lines and the grid, and held there as a move is (see
/// [`RELEASE`]). `keep_aspect`: a picture's corner keeps its proportions, so only the edge
/// nearer a guide snaps and the other follows. Alt held resizes freely.
pub(crate) fn snap_resize(ui: &egui::Ui, page: Rect, from: Rect, to: Rect, x: Option<bool>, y: Option<bool>, keep_aspect: bool) -> Rect {
    let ctx = ui.ctx();
    if ui.input(|i| i.modifiers.alt) || !guide_options(ctx).1 {
        return to;
    }
    let grid = ctx.data(|g| g.get_temp::<Option<f32>>(grid_id(page))).flatten();
    let id = egui::Id::unique("snap-hold-resize");
    let now = ui.input(|i| i.time);
    let mut hold = ctx.data(|g| g.get_temp::<(Hold, f64)>(id)).filter(|(_, at)| now - at <= HOLD_FORGET).map(|(h, _)| h).unwrap_or_default();
    let out = snap_resized_held(page, to, (x, y), keep_aspect, &snap_targets(ctx, page, from), (grid, &snap_lines(ctx, page)), &mut hold);
    ctx.data_mut(|g| g.insert_temp(id, (hold, now)));
    out
}

/// [`snap_resize`] without the Alt key, lining up with `others`, held in `hold`.
fn snap_resized_held(
    page: Rect,
    to: Rect,
    (x, y): (Option<bool>, Option<bool>),
    keep_aspect: bool,
    others: &[Rect],
    (grid, (rx, ry)): (Option<f32>, &(Vec<f32>, Vec<f32>)),
    hold: &mut Hold,
) -> Rect {
    if !(to.is_positive() && to.is_finite()) {
        return to;
    }
    let near = |origin: f32, edge: f32| grid.filter(|s| s.is_finite() && *s >= 1.0).map(|s| origin + ((edge - origin) / s).round() * s);
    let edge_x = x.map(|left| if left { to.left() } else { to.right() });
    let edge_y = y.map(|top| if top { to.top() } else { to.bottom() });
    let sx = edge_x.and_then(|e| {
        let guides =
            std::iter::once(page.center().x).chain(others.iter().flat_map(|o| lines_x(*o))).chain(near(page.left(), e)).chain(rx.iter().copied());
        snap_axis(&[e], guides, &mut hold.x)
    });
    let sy = edge_y.and_then(|e| {
        let guides =
            std::iter::once(page.center().y).chain(others.iter().flat_map(|o| lines_y(*o))).chain(near(page.top(), e)).chain(ry.iter().copied());
        snap_axis(&[e], guides, &mut hold.y)
    });
    if x.is_none() {
        hold.x = None;
    }
    if y.is_none() {
        hold.y = None;
    }
    if let (true, Some(left), Some(top)) = (keep_aspect, x, y) {
        // The nearer guide wins; the other side follows from the box's proportions, around the
        // corner opposite the one dragged.
        let (w, h) = (to.width(), to.height());
        let wide = |a: f32| {
            let nw = w + if left { -a } else { a };
            (nw, h * nw / w)
        };
        let tall = |b: f32| {
            let nh = h + if top { -b } else { b };
            (w * nh / h, nh)
        };
        let (nw, nh) = match (sx, sy) {
            (Some(a), Some(b)) if a.abs() <= b.abs() => wide(a),
            (Some(a), None) => wide(a),
            (_, Some(b)) => tall(b),
            (None, None) => return to,
        };
        if !(nw.is_finite() && nh.is_finite() && nw >= 1.0 && nh >= 1.0) {
            return to;
        }
        let (ax, ay) = (if left { to.right() } else { to.left() }, if top { to.bottom() } else { to.top() });
        let corner = Pos2::new(if left { ax - nw } else { ax + nw }, if top { ay - nh } else { ay + nh });
        return Rect::from_two_pos(Pos2::new(ax, ay), corner);
    }
    let mut out = to;
    match (x, sx) {
        (Some(true), Some(s)) => out.min.x += s,
        (Some(false), Some(s)) => out.max.x += s,
        _ => {}
    }
    match (y, sy) {
        (Some(true), Some(s)) => out.min.y += s,
        (Some(false), Some(s)) => out.max.y += s,
        _ => {}
    }
    if out.width() >= 1.0 && out.height() >= 1.0 { out } else { to }
}

/// The guides a box moved from `from` to `r` sits on: the page's centre lines across the page,
/// and a line from it to each other box it lines up with.
pub(crate) fn paint_guides(painter: &egui::Painter, page: Rect, from: Rect, r: Rect) {
    if !guide_options(painter.ctx()).0 {
        return;
    }
    let stroke = Stroke::new(1.0, GUIDE);
    let centre = Stroke::new(1.0, CENTRE_GUIDE);
    let on = |a: f32, b: f32| (a - b).abs() < 0.5;
    let pc = page.center();
    if on(r.center().x, pc.x) {
        painter.line_segment([Pos2::new(pc.x, page.top()), Pos2::new(pc.x, page.bottom())], centre);
    }
    if on(r.center().y, pc.y) {
        painter.line_segment([Pos2::new(page.left(), pc.y), Pos2::new(page.right(), pc.y)], centre);
    }
    for o in snap_targets(painter.ctx(), page, from) {
        for x in lines_x(o) {
            if lines_x(r).into_iter().any(|m| on(m, x)) {
                painter.line_segment([Pos2::new(x, r.top().min(o.top())), Pos2::new(x, r.bottom().max(o.bottom()))], stroke);
            }
        }
        for y in lines_y(o) {
            if lines_y(r).into_iter().any(|m| on(m, y)) {
                painter.line_segment([Pos2::new(r.left().min(o.left()), y), Pos2::new(r.right().max(o.right()), y)], stroke);
            }
        }
    }
}

/// How far a box has been moved, in points, beside the pointer `at` (screen move `d`), as
/// Photoshop shows it (Thai edition).
pub(crate) fn paint_offset(painter: &egui::Painter, xf: &PageXform, at: Pos2, d: egui::Vec2) {
    if !guide_options(painter.ctx()).0 {
        return;
    }
    let ((x0, y0), (x1, y1)) = (xf.screen_to_view(at - d), xf.screen_to_view(at));
    let (dx, dy) = (x1 - x0, y1 - y0);
    if !(dx.is_finite() && dy.is_finite()) || d.length() < 1.0 {
        return;
    }
    let text = format!("\u{394}X: {dx:.0} pt\n\u{394}Y: {dy:.0} pt");
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
    let rect = Rect::from_min_size(at + egui::vec2(16.0, 16.0), galley.size() + egui::vec2(12.0, 8.0));
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_rgba_unmultiplied(40, 40, 40, 230));
    painter.galley(rect.min + egui::vec2(6.0, 4.0), galley, Color32::WHITE);
}

/// The boxes of what is on `page` besides the item moved, for [`set_snap_targets`]: added items,
/// the page's paragraphs and pictures.
pub(crate) fn page_boxes(
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    added: &[Added],
    blocks: &[pdfcraft_engine::TextBlock],
    images: &[pdfcraft_engine::PageImage],
) -> Vec<Rect> {
    on_page(added, page)
        .into_iter()
        .map(|(_, a)| screen_rect(xf, info, page, a.content.rect()))
        .chain(blocks.iter().map(|b| xf.user_rect_f64(info, page, b.rect)))
        .chain(images.iter().map(|im| crate::canvas::image_screen_rect(im, xf, info, page)).filter(|r| !backdrop(xf.rect, *r)))
        .filter(|r| r.is_positive() && r.width().is_finite() && r.height().is_finite())
        .collect()
}

/// A picture covering most of the page (`page`, screen): a scan or a background, whose centre
/// lies a hair off the page's. Lined up with, it caught boxes just beside the page's centre
/// lines, in magenta, before they reached the green ones (Thai edition).
fn backdrop(page: Rect, r: Rect) -> bool {
    let covered = r.intersect(page);
    covered.is_positive() && covered.area() >= 0.8 * page.area()
}

/// The rect while moving or resizing (images keep their proportions).
fn dragged(r: Rect, grab: Grab, d: egui::Vec2, keep_aspect: bool) -> Rect {
    match grab {
        Grab::Move => r.translate(d),
        Grab::Corner(left, top) => {
            let (mut dx, mut dy) = (if left { -d.x } else { d.x }, if top { -d.y } else { d.y });
            if keep_aspect && r.width() > 0.0 && r.height() > 0.0 {
                let k = ((r.width() + dx) / r.width()).max((r.height() + dy) / r.height()).max(0.05);
                dx = r.width() * k - r.width();
                dy = r.height() * k - r.height();
            }
            let mut out = r;
            if left {
                out.min.x -= dx;
            } else {
                out.max.x += dx;
            }
            if top {
                out.min.y -= dy;
            } else {
                out.max.y += dy;
            }
            Rect::from_two_pos(out.min, out.max)
        }
    }
}

/// What of the added text and pictures lies under `p` on `page`: the selected item's corners, then
/// the topmost item. Placed on the page, over a form field under it.
pub(crate) fn hit(xf: &PageXform, info: &DocInfo, page: usize, added: &[Added], view: &DocView, p: Pos2) -> Option<Hit> {
    if !xf.rect.contains(p) {
        return None;
    }
    let items: Vec<(usize, &Added)> = on_page(added, page).into_iter().filter(|(_, a)| matches!(a.content, AddedContent::Text(_))).collect();
    let selected = view.content.selected.filter(|s| s.0 == page).and_then(|(_, si)| items.iter().find(|(i, _)| *i == si));
    if let Some((index, a)) = selected
        && let Some((_, left, top)) = handles(screen_rect(xf, info, page, a.content.rect())).into_iter().find(|(c, ..)| c.distance(p) <= 6.0)
    {
        let icon = if left == top { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw };
        return Some(Hit::new(Owner::Added { index: *index, corner: Some((left, top)) }, Layer::Handle, icon));
    }
    let (index, _) = items.iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(p))?;
    Some(Hit::new(Owner::Added { index: *index, corner: None }, Layer::Placed, egui::CursorIcon::Move))
}

/// Pointer input on one page in Edit mode (`adding_text` with the Text tool). `true` when it
/// used the input.
pub(crate) fn page_input(
    cx: PageToolContext<'_>,
    added: &[Added],
    adding_text: bool,
    edit_on_click: bool,
    style: &AddedText,
    view: &mut DocView,
) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, info, .. } = cx;
    let mut consumed = false;
    let pointer = ptr.pos;
    // The other page under the pointer, where a dragged item lands (Thai edition).
    let drop_other = pointer.and_then(|p| view.other_page_at(page, p));
    let cv = &mut view.content;
    if let Some((gp, grab, start)) = cv.grab
        && gp == page
    {
        consumed = true;
        if resp.drag_stopped() || !ptr.primary_down {
            cv.grab = None;
            let end = pointer.unwrap_or(start);
            if let Some((sp, si)) = cv.selected
                && sp == page
                && let Some((_, a)) = on_page(added, page).into_iter().find(|(i, a)| *i == si && matches!(a.content, AddedContent::Text(_)))
                && (end - start).length() >= 1.0
            {
                let keep = matches!(a.content, AddedContent::Image(_));
                let from = screen_rect(xf, info, page, a.content.rect());
                // Dropped on another page (Thai edition): it goes there, where it was dropped.
                let other = (grab == Grab::Move).then_some(drop_other).flatten();
                let (to_page, txf) = other.unwrap_or((page, *xf));
                let d = match (grab, other) {
                    (Grab::Move, None) => snap_to_centre(ui, xf.rect, from, end - start),
                    _ => end - start,
                };
                let mut r = dragged(from, grab, d, keep);
                if let Grab::Corner(left, top) = grab {
                    r = snap_resize(ui, xf.rect, from, r, Some(left), Some(top), keep);
                }
                let (p0, p1) = (to_display(&txf, info, to_page, r.left_bottom()), to_display(&txf, info, to_page, r.right_top()));
                let mut rect = [p0[0], p0[1], p1[0], p1[1]];
                if let AddedContent::Text(t) = &a.content
                    && grab == Grab::Move
                {
                    // Text height follows its lines: keep the top edge's offset only.
                    rect = [rect[0], rect[3] - (t.rect[3] - t.rect[1]), rect[2], rect[3]];
                }
                // Ctrl held as a text box is dropped leaves it where it was and adds a copy there
                // (Thai edition), as in Word.
                let copying = ui.input(|i| i.modifiers.command);
                view.pending_edit = Some(match &a.content {
                    AddedContent::Text(t) if copying && grab == Grab::Move && to_page == page => {
                        cv.select_added = Some((page, on_page(added, page).len()));
                        Edit::AddText { page, text: AddedText { rect, ..t.clone() } }
                    }
                    // A picture copied, or anything taken to another page: the item there (Thai
                    // edition), selected.
                    _ if grab == Grab::Move && (to_page != page || copying) => {
                        cv.select_added = Some((to_page, on_page(added, to_page).len()));
                        cv.selected = None;
                        Edit::MoveContentToPage { page, index: si, to_page, content: a.content.with_rect(rect), copy: copying }
                    }
                    _ => Edit::UpdateContent { page, index: si, content: a.content.with_rect(rect) },
                });
            }
        }
        return consumed;
    }
    // A right-click on an item selects it, and the page's menu shows its items (Arrange, Delete;
    // see [`page_menu`]) (Thai edition).
    if resp.secondary_clicked()
        && let Some(p) = pointer.filter(|p| xf.rect.contains(*p))
    {
        // Pictures have the image controller's menu.
        let hit = on_page(added, page)
            .into_iter()
            .rev()
            .filter(|(_, a)| matches!(a.content, AddedContent::Text(_)))
            .find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(p));
        if let Some((i, _)) = hit
            && cv.draft.is_none()
        {
            cv.selected = Some((page, i));
            cv.menu = Some((page, i));
            consumed = true;
        }
    }
    // A drag starts where the button went down, which the pointer may already have left by the
    // time egui calls it a drag (a quick drag toward another page; Thai edition).
    if resp.drag_started()
        && cv.draft.is_none()
        && let Some(origin) = ptr.origin.filter(|o| xf.rect.contains(*o))
    {
        // Pictures are the image controller's.
        let items: Vec<(usize, &Added)> = on_page(added, page).into_iter().filter(|(_, a)| matches!(a.content, AddedContent::Text(_))).collect();
        let selected_rect = cv
            .selected
            .filter(|s| s.0 == page)
            .and_then(|(_, si)| items.iter().find(|(i, _)| *i == si))
            .map(|(_, a)| screen_rect(xf, info, page, a.content.rect()));
        if let Some((_, left, top)) = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(origin) <= 6.0)) {
            cv.grab = Some((page, Grab::Corner(left, top), origin));
            return true;
        }
        if let Some((i, _)) = items.iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(origin)) {
            cv.selected = Some((page, *i));
            cv.grab = Some((page, Grab::Move, origin));
            return true;
        }
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return consumed };
    let items: Vec<(usize, &Added)> = on_page(added, page).into_iter().filter(|(_, a)| matches!(a.content, AddedContent::Text(_))).collect();
    let hit = items.iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(p)).map(|(i, a)| (*i, *a));
    let selected_rect = cv
        .selected
        .filter(|s| s.0 == page)
        .and_then(|(_, si)| items.iter().find(|(i, _)| *i == si))
        .map(|(_, a)| screen_rect(xf, info, page, a.content.rect()));
    let corner = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(p) <= 6.0));
    if adding_text && hit.is_none() {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Text);
        // A new box where nothing drawn over the page holds the press.
        if resp.clicked() && view.router.owner(page).is_none() {
            // Text that can't be drawn stays where it is being typed (#125).
            if cv.hold_blocked() {
                return true;
            }
            // A click elsewhere keeps the text being typed and starts a new box (#74). The click
            // can reach the page before the editor sees it lose focus, so finish it here.
            let typed = cv.draft.take().and_then(|d| finish(cv, d, added, info));
            // The Format panel styles the new text, not the box just kept.
            (cv.selected, cv.select_added) = (None, None);
            let at = to_display(xf, info, page, p);
            let mut style = style.clone();
            style.text.clear();
            let page_width = info.pages.get(page).map_or(f64::INFINITY, |p| p.width as f64);
            let rect = [at[0], at[1] - style.size * 1.2, (at[0] + 200.0).min(page_width), at[1]];
            style.rect = rect;
            cv.draft =
                Some(TextDraft { page, index: None, rect, text: String::new(), style, focus: true, auto: true, selection: None, typing_from: None });
            consumed = true;
            if typed.is_some() {
                view.pending_edit = typed;
            }
        }
        return consumed;
    }
    // A drag on what the press was on ([`hit`]): a corner resizes the selected item, the item
    // itself moves.
    if resp.drag_started() {
        let origin = ptr.origin.unwrap_or(p);
        match view.router.owner(page) {
            Some(Owner::Added { corner: Some((left, top)), .. }) => {
                cv.grab = Some((page, Grab::Corner(left, top), origin));
                consumed = true;
            }
            Some(Owner::Added { index, corner: None }) => {
                cv.selected = Some((page, index));
                cv.grab = Some((page, Grab::Move, origin));
                consumed = true;
            }
            _ => {}
        }
        return consumed;
    }
    if (resp.double_clicked() || edit_on_click && resp.clicked() && corner.is_none())
        && let Some((i, a)) = hit
        && let AddedContent::Text(t) = &a.content
    {
        if cv.hold_blocked() {
            return true;
        }
        if cv.draft.as_ref().is_some_and(|d| d.page == page && d.index == Some(i)) {
            return true;
        }
        if let Some(edit) = cv.finish_draft(added, info) {
            view.pending_edit = Some(edit);
        }
        // A box with lines aligned on their own keeps its width: they are aligned in it. So does a
        // box made wider than its text (Thai edition): its text is aligned inside it.
        let page_width = info.pages.get(page).map_or(f64::INFINITY, |p| f64::from(p.width));
        let auto = pdfcraft_engine::hugs_text(t, page_width) && t.aligns.iter().all(Option::is_none)
            || t.direction != pdfcraft_engine::TextDirection::Horizontal;
        cv.draft = Some(TextDraft {
            page,
            index: Some(i),
            rect: t.rect,
            text: t.text.clone(),
            style: t.clone(),
            focus: true,
            auto,
            selection: None,
            typing_from: None,
        });
        cv.selected = Some((page, i));
        consumed = true;
        return consumed;
    }
    if resp.clicked() {
        match hit {
            Some((i, _)) => {
                cv.selected = Some((page, i));
                consumed = true;
            }
            None if corner.is_none() => cv.selected = None,
            None => consumed = true,
        }
    }
    consumed || hit.is_some() && resp.is_pointer_button_down_on()
}

/// What the right-click menu of an added item chose.
enum ItemChoice {
    Arrange(Arrange),
    Delete,
}

/// The Arrange orders with their shortcuts (Word's and PowerPoint's keys).
const ARRANGE: [(Arrange, &str); 4] =
    [(Arrange::Front, "Ctrl+Shift+]"), (Arrange::Forward, "Ctrl+]"), (Arrange::Backward, "Ctrl+["), (Arrange::Back, "Ctrl+Shift+[")];

/// The page's right-click menu over an added text box or image (Arrange, Delete): `true` when
/// it was shown, so the page's other menus are not. Its choice becomes the pending edit.
pub(crate) fn page_menu(ui: &mut egui::Ui, view: &mut DocView, doc: &pdfcraft_engine::Document) -> bool {
    let Some((page, index)) = view.content.menu.filter(|m| view.content.selected == Some(*m) && view.content.draft.is_none()) else {
        return false;
    };
    let Some((_, a)) = on_page(doc.added(), page).into_iter().find(|(i, _)| *i == index) else { return false };
    let (layer, covered) = (doc.added_layer(page, index), doc.comments_over(page, index));
    match item_menu(ui, |how| arrange_moves(layer, how) || how == Arrange::Front && covered > 0) {
        Some(ItemChoice::Arrange(how)) => {
            view.content.follow = Some((page, a.obj));
            view.pending_edit = Some(arrange_edit(page, index, how, covered));
        }
        Some(ItemChoice::Delete) => {
            (view.content.selected, view.content.menu) = (None, None);
            view.pending_edit = Some(Edit::DeleteContent { page, index });
        }
        None => {}
    }
    // Thai edition: a text box made text of the page, where a part of it can take its own colour
    // or size (double-click it, select the part, then Format text).
    if matches!(a.content, AddedContent::Text(_)) && ui.button(tl!("Convert to page text")).clicked() {
        (view.content.selected, view.content.menu) = (None, None);
        view.pending_edit = Some(Edit::ReleaseContent { page, index });
        ui.close();
    }
    true
}

/// The edit for Arrange `how` on added item `index` of `page`: in front of everything takes it
/// over the `covered` comments too, which go onto the page first.
fn arrange_edit(page: usize, index: usize, how: Arrange, covered: usize) -> Edit {
    if how == Arrange::Front && covered > 0 { Edit::BringOverComments { page, index } } else { Edit::ArrangeContent { page, index, how } }
}

/// Whether `how` moves an item at `layer` (its layer, and how many the page has).
pub(crate) fn arrange_moves(layer: Option<(usize, usize)>, how: Arrange) -> bool {
    layer.is_some_and(|(at, count)| match how {
        Arrange::Front | Arrange::Forward => at + 1 < count,
        Arrange::Backward | Arrange::Back => at > 0,
    })
}

/// The Arrange submenu: the four orders, those that would move nothing greyed out.
pub(crate) fn arrange_menu(ui: &mut egui::Ui, moves: impl Fn(Arrange) -> bool) -> Option<Arrange> {
    let mut chosen = None;
    ui.menu_button(tl!("Arrange"), |ui| {
        for (how, keys) in ARRANGE {
            let label = match how {
                Arrange::Front => tl!("Bring to front"),
                Arrange::Forward => tl!("Bring forward"),
                Arrange::Backward => tl!("Send backward"),
                Arrange::Back => tl!("Send to back"),
            };
            if ui.add_enabled(moves(how), egui::Button::new(label).shortcut_text(keys)).clicked() {
                chosen = Some(how);
                ui.close();
            }
        }
    });
    chosen
}

/// The right-click menu of an added text box or image: Arrange and Delete.
fn item_menu(ui: &mut egui::Ui, moves: impl Fn(Arrange) -> bool) -> Option<ItemChoice> {
    let mut chosen = arrange_menu(ui, moves).map(ItemChoice::Arrange);
    ui.separator();
    if ui.button(tl!("Delete")).clicked() {
        chosen = Some(ItemChoice::Delete);
        ui.close();
    }
    chosen
}

/// The Arrange order the keys pressed this frame ask for: Ctrl+] forward, Ctrl+[ backward, with
/// Shift to the front or back. The key's place on the keyboard counts, so it works on a Thai
/// layout too, where those keys type Thai letters.
pub(crate) fn arrange_key(ctx: &egui::Context) -> Option<Arrange> {
    ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt => {
                match (physical_key.unwrap_or(*key), modifiers.shift) {
                    (egui::Key::CloseBracket, false) => Some(Arrange::Forward),
                    (egui::Key::CloseBracket, true) => Some(Arrange::Front),
                    (egui::Key::OpenBracket, false) => Some(Arrange::Backward),
                    (egui::Key::OpenBracket, true) => Some(Arrange::Back),
                    _ => None,
                }
            }
            _ => None,
        })
    })
}

/// Outlines of added items (hover and selection with handles) and the box being dragged, the
/// pages' `pointer` over them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_page(
    ui: &egui::Ui,
    painter: &egui::Painter,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    added: &[Added],
    view: &DocView,
    pointer: Option<Pos2>,
) {
    let cv = &view.content;
    for (i, a) in on_page(added, page) {
        if matches!(a.content, AddedContent::Image(_)) {
            continue;
        }
        if cv.draft.as_ref().is_some_and(|d| d.page == page && d.index == Some(i)) {
            continue;
        }
        let mut r = screen_rect(xf, info, page, a.content.rect());
        let selected = cv.selected == Some((page, i));
        if selected
            && let (Some((gp, g, start)), Some(p)) = (cv.grab, pointer)
            && gp == page
        {
            let picture = matches!(a.content, AddedContent::Image(_));
            let from = r;
            match g {
                Grab::Move => {
                    let d = snap_to_centre(ui, xf.rect, r, p - start);
                    r = dragged(r, g, d, picture);
                    paint_offset(painter, xf, p, d);
                }
                Grab::Corner(left, top) => r = snap_resize(ui, xf.rect, from, dragged(r, g, p - start, picture), Some(left), Some(top), picture),
            }
            paint_guides(painter, xf.rect, from, r);
        }
        if selected
            && cv.grab.is_none()
            && cv.draft.is_none()
            && let AddedContent::Text(t) = &a.content
            && let Some(previewed) = paint_look_preview(ui, painter, xf, page, info, t)
        {
            r = previewed;
        }
        if selected {
            painter.rect_stroke(r.expand(1.0), CornerRadius::ZERO, Stroke::new(1.5, SELECT_BLUE), egui::StrokeKind::Outside);
            for (c, ..) in handles(r.expand(1.0)) {
                painter.circle(c, 3.5, Color32::WHITE, Stroke::new(1.5, SELECT_BLUE));
            }
        } else if pointer.is_some_and(|p| r.expand(2.0).contains(p)) {
            painter.rect_stroke(r.expand(1.0), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
}

/// An added picture dragged from `page` and dropped on another one (`preview`: where it lies on
/// screen, `p`: the pointer) goes there, or with Ctrl a copy of it does (Thai edition).
pub(crate) fn picture_dropped(
    ui: &egui::Ui,
    view: &mut DocView,
    added: &[Added],
    page: usize,
    info: &DocInfo,
    preview: Rect,
    p: Pos2,
) -> Option<Edit> {
    let (to_page, txf) = view.other_page_at(page, p)?;
    let (_, index) = view.content.selected.filter(|s| s.0 == page)?;
    let (_, a) = on_page(added, page).into_iter().find(|(i, a)| *i == index && matches!(a.content, AddedContent::Image(_)))?;
    let (p0, p1) = (to_display(&txf, info, to_page, preview.left_bottom()), to_display(&txf, info, to_page, preview.right_top()));
    let content = a.content.with_rect([p0[0], p0[1], p1[0], p1[1]]);
    view.content.select_added = Some((to_page, on_page(added, to_page).len()));
    view.content.selected = None;
    Some(Edit::MoveContentToPage { page, index, to_page, content, copy: ui.input(|i| i.modifiers.command) })
}

/// The page's own picture `index`, dragged from `page` (`target`: its whole box on screen, `p`:
/// the pointer) and dropped on another page, goes there, or with Ctrl a copy of it does; with
/// Ctrl, a drop on its own page leaves it and adds the copy there (Thai edition). `None`: a plain
/// drop on its own page, which the caller moves.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_picture_dropped(
    ui: &egui::Ui,
    view: &mut DocView,
    xf: &PageXform,
    page: usize,
    index: usize,
    info: &DocInfo,
    target: Rect,
    p: Pos2,
) -> Option<Edit> {
    let copy = ui.input(|i| i.modifiers.command);
    // Another page takes it (moved, or copied with Ctrl); on its own page only Ctrl makes a copy,
    // as with added pictures and text (Thai edition); a plain move there is the caller's.
    let (to_page, txf) = match view.other_page_at(page, p) {
        Some(other) => other,
        None if copy => (page, *xf),
        None => return None,
    };
    let (p0, p1) = (to_display(&txf, info, to_page, target.left_bottom()), to_display(&txf, info, to_page, target.right_top()));
    view.image_selection = None;
    Some(Edit::MovePageImageToPage { page, index, to_page, rect: [p0[0], p0[1], p1[0], p1[1]], copy })
}

/// The image controller owns image selection; the added-content panel follows the same XObject.
pub(crate) fn sync_image_selection(view: &mut DocView, page: usize, images: &[pdfcraft_engine::PageImage], added: &[Added]) {
    let Some(selection) = view.image_selection.as_ref().filter(|selection| selection.page == page) else { return };
    if view.content.draft.is_some() {
        return;
    }
    let object = images
        .get(selection.index)
        .and_then(|image| image.object)
        .filter(|object| images.iter().filter(|image| image.object == Some(*object)).count() == 1);
    view.content.selected = object.and_then(|object| {
        let mut matches = added
            .iter()
            .filter(|item| item.page == page)
            .enumerate()
            .filter(|(_, item)| matches!(&item.content, AddedContent::Image(image) if image.image == object));
        let selected = matches.next().map(|(index, _)| (page, index));
        if matches.next().is_none() { selected } else { None }
    });
    view.content.select_added = None;
    view.content.grab = None;
}

/// Leaving the image controller cannot leave a hidden added image owning keyboard actions.
pub(crate) fn release_image_selection(view: &mut DocView, added: &[Added]) {
    if view.content.draft.is_some() {
        return;
    }
    let image_selected = view.content.selected.is_some_and(|(page, index)| {
        added.iter().filter(|item| item.page == page).nth(index).is_some_and(|item| matches!(item.content, AddedContent::Image(_)))
    });
    if image_selected {
        view.content.selected = None;
        view.content.select_added = None;
        view.content.grab = None;
    }
}

/// Delete removes the selected item; Escape clears the selection; the arrow keys move it.
pub(crate) fn keys(
    ctx: &egui::Context,
    view: &mut DocView,
    added: &[Added],
    (layer, covered): (Option<(usize, usize)>, usize),
    image_controller_active: bool,
) {
    if image_controller_active && view.image_selection.is_some()
        || matches!(view.pending_edit.as_ref(), Some(Edit::EditPageImage { .. }))
        || view.content.selected.is_none()
        || view.content.draft.is_some()
        || ctx.egui_wants_keyboard_input()
    {
        return;
    }
    let (del, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Escape)));
    if del && let Some((page, index)) = view.content.selected.take() {
        view.pending_edit = Some(Edit::DeleteContent { page, index });
    } else if let Some(how) = arrange_key(ctx)
        && view.pending_edit.is_none()
        && (arrange_moves(layer, how) || how == Arrange::Front && covered > 0)
        && let Some((page, index)) = view.content.selected
        && let Some((_, a)) = on_page(added, page).into_iter().find(|(i, _)| *i == index)
    {
        view.content.follow = Some((page, a.obj));
        view.pending_edit = Some(arrange_edit(page, index, how, covered));
    } else if esc {
        view.content.selected = None;
    } else if let Some([dx, dy]) = nudge(ctx)
        && view.pending_edit.is_none()
        && let Some((page, index)) = view.content.selected
        && let Some((_, a)) = on_page(added, page).into_iter().find(|(i, _)| *i == index)
    {
        // Display space: y grows up the page.
        let r = a.content.rect();
        let content = a.content.with_rect([r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]);
        view.pending_edit = Some(Edit::UpdateContent { page, index, content });
    }
}

/// Whether the arrow keys move the selected added item or the Edit text selection (the page
/// keeps still meanwhile).
pub(crate) fn arrows_nudge(view: &DocView) -> bool {
    view.content.selected.is_some() && view.content.draft.is_none() || view.objects.count() > 0 || view.image_selection.is_some()
}

/// How far the arrow keys pressed this frame move the selected item, in points: one point a
/// press, ten with Shift. Each press is one undo step.
pub(crate) fn nudge(ctx: &egui::Context) -> Option<[f64; 2]> {
    use egui::Key;
    ctx.input(|i| {
        if i.modifiers.command || i.modifiers.alt {
            return None;
        }
        let step = if i.modifiers.shift { 10.0 } else { 1.0 };
        let count = |k| i.num_presses(k) as f64;
        let dx = (count(Key::ArrowRight) - count(Key::ArrowLeft)) * step;
        let dy = (count(Key::ArrowUp) - count(Key::ArrowDown)) * step;
        (dx != 0.0 || dy != 0.0).then_some([dx, dy])
    })
}

/// After an add: select the new item.
pub(crate) fn after_refresh(view: &mut DocView, added: &[Added]) {
    // After Arrange: the same item, at its new index.
    if let Some((page, obj)) = view.content.follow.take()
        && let Some((i, _)) = on_page(added, page).into_iter().find(|(_, a)| a.obj == obj)
    {
        view.content.selected = Some((page, i));
    }
    if let Some((page, before)) = view.content.select_added.take()
        && on_page(added, page).len() > before
    {
        view.content.selected = Some((page, before));
    }
    if let Some((p, i)) = view.content.selected
        && on_page(added, p).len() <= i
    {
        view.content.selected = None;
    }
}

/// The added-text editor's inset on either side (screen points).
const EDITOR_MARGIN: f32 = 4.0;

/// The in-place text editor, with Done and Discard beside it. Returns the edit once the text is
/// committed (a click away, or Done), and whether Done finished adding text. Discard or Escape
/// throws the draft away.
pub(crate) fn editor(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, added: &[Added]) -> (Option<Edit>, bool) {
    let doc_id = view.id;
    let Some(page) = view.content.draft.as_ref().map(|d| d.page) else { return (None, false) };
    let Some(xf) = view.page_xform(page) else { return (None, false) };
    let zoom = xf.zoom();
    let Some(draft) = view.content.draft.as_mut() else { return (None, false) };
    let font = crate::edit_text_ui::editor_font(ctx, &draft.style, (draft.style.size as f32 * zoom).clamp(1.0, 300.0));
    // Vertical text is typed across, as it reads (Thai edition); the page turns it once committed.
    let flat = AddedText { direction: pdfcraft_engine::TextDirection::Horizontal, ..draft.style.clone() };
    let text = AddedText { text: draft.text.clone(), ..flat.clone() };
    let Some(page_width) = info.pages.get(page).map(|p| p.width as f64) else { return (None, false) };
    if let Some((key, z, width, rect)) = &view.content.layout
        && *key == text
        && *z == zoom
        && *width == page_width
    {
        draft.rect = *rect;
    } else {
        let source = draft.style.rect;
        if draft.index.is_some() && draft.text == draft.style.text && !draft.auto {
            draft.rect = source;
        } else {
            let natural = ctx.fonts_mut(|f| f.layout_no_wrap(draft.text.clone(), font.clone(), Color32::BLACK).size().x) as f64 / zoom as f64;
            let width = natural.max(pdfcraft_engine::added_text_width(&text)) + 2.0;
            // A box that hugs its text follows it while it is typed, narrower as well as wider
            // (Thai edition), keeping the edge its alignment holds; a wrapped box keeps its width.
            let (width, x0) = if draft.auto {
                let x0 = match draft.style.align {
                    pdfcraft_engine::TextAlign::Right => source[2] - width,
                    pdfcraft_engine::TextAlign::Center => (source[0] + source[2] - width) / 2.0,
                    pdfcraft_engine::TextAlign::Left | pdfcraft_engine::TextAlign::Justify => source[0],
                };
                let x0 = x0.clamp(0.0, (page_width - width).max(0.0));
                (width.min((page_width - x0).max(1.0)), x0)
            } else {
                (width.max(source[2] - source[0]).min((page_width - source[0]).max(1.0)), source[0])
            };
            draft.rect = [x0, source[1], x0 + width, source[3]];
            draft.rect = pdfcraft_engine::added_text_rect(&AddedText { rect: draft.rect, ..text.clone() });
        }
        view.content.layout = Some((text, zoom, page_width, draft.rect));
    }
    let r = screen_rect(&xf, info, page, draft.rect);
    let source_rect = draft.index.map(|_| screen_rect(&xf, info, page, draft.style.rect));
    // At least 60 points wide, so a short box can still be clicked into. Where that is wider than
    // the box, the editor reaches past it away from the edge its text keeps (the right edge for
    // right-aligned text, both halves for centred), so the text stays where the page draws it.
    // A box that hugs its text is not wrapped by the editor while it fits beside the page's edge
    // (Thai edition): the screen font measures a little differently from the page's, so the box
    // takes the editor's margins and the text lays out on one line per line typed. A box that
    // has reached the edge wraps there, as before.
    let room = info.pages.get(page).map_or(0.0, |p| f64::from(p.width)) - draft.rect[0];
    let hugs = draft.auto && draft.rect[2] - draft.rect[0] < room - 0.5;
    // shortcut: lines aligned differently from each other in a box under 60 points keep the
    // editor's left edge; anchor each line on its own if such boxes turn out to matter.
    let width = if hugs { r.width() + 2.0 * EDITOR_MARGIN } else { r.width() }.max(60.0);
    // The alignment every line shares, its own or the box's.
    let first = flat.paragraph_align(0);
    let shared = (1..pdfcraft_engine::paragraph_align::count(&draft.text)).all(|i| flat.paragraph_align(i) == first).then_some(first);
    let anchor = match shared {
        Some(pdfcraft_engine::TextAlign::Center) => 0.5,
        Some(pdfcraft_engine::TextAlign::Right) => 1.0,
        _ => 0.0,
    };
    let at = r.min - egui::vec2((width - r.width()) * anchor, 0.0);
    let background_ready = draft.index.is_some_and(|index| view.text_background.ready_added(page, index));
    let tokens = Tokens::get(ctx);
    let (mut commit, mut cancel, mut done) = (false, false, false);
    // A right-click on the box with nothing typed or restyled in it (Thai edition).
    let mut menu_at: Option<Pos2> = None;
    let mut undrawable = None;
    let memo = &view.content.undrawable;
    // Clicks outside the document (the Format text panel) keep the text open, as they keep a
    // paragraph being edited open.
    // Where the click that took the focus was: the pointer itself may be gone by then (a touch, a
    // pen lifted away). The keyboard moving there (Tab, a screen reader) keeps it open too.
    let away = |p: egui::Pos2| !view.viewport_rect().contains(p);
    let outside = ctx.input(|i| i.pointer.interact_pos()).is_some_and(away) || crate::edit_text_ui::focus_pos(ctx).is_some_and(away);
    // Shown at once, like the paragraph editor: a fade-in shows the page's copy through it. Over
    // the page, so only where the document shows: text scrolled under the mode bar or a side
    // panel must leave them their pointer (#2755).
    egui::Area::new(egui::Id::unique(("added-text", view.id.0)))
        .fade_in(false)
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .constrain_to(view.viewport_rect())
        .constrain(false)
        .show(ctx, |ui| {
            let Some(t) = view.content.draft.as_mut() else { return };
            ui.set_width(r.width() + 64.0);
            if let Some(source_rect) = source_rect.filter(|_| !background_ready) {
                ui.painter().rect_filled(source_rect.union(r).expand(1.0), CornerRadius::ZERO, Color32::WHITE);
            }
            // The box is white in every theme, so the caret must not follow the theme (pale in dark mode) (#655).
            ui.visuals_mut().text_cursor.stroke.color = crate::edit_text_ui::caret_color_on(Color32::WHITE);
            let [cr, cg, cb] = t.style.color.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
            let color = Color32::from_rgb(cr, cg, cb);
            // Character spacing shows as the page will draw it, scaled like the font.
            let letter_spacing = (t.style.char_spacing * f64::from(zoom)) as f32;
            // Laid out as the page draws it, each paragraph with its own alignment, so opening the
            // box doesn't show it realigned.
            let style = AddedText { direction: pdfcraft_engine::TextDirection::Horizontal, ..t.style.clone() };
            let line_font = font.clone();
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                aligned_galley(ui, text.as_str(), &style, &line_font, color, letter_spacing, wrap, hugs)
            };
            let before = t.text.clone();
            let input = input_id();
            t.claim_typing(ui.ctx(), doc_id);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let output = egui::TextEdit::multiline(&mut t.text)
                    .font(font)
                    .desired_width(width)
                    .desired_rows(1)
                    .layouter(&mut layouter)
                    // Keep the page editor's original inset; the theme's larger button padding
                    // otherwise shifts narrow right-aligned boxes away from their text edge.
                    .margin(egui::Margin::symmetric(EDITOR_MARGIN as i8, 2))
                    .background_color(if background_ready { Color32::TRANSPARENT } else { Color32::WHITE })
                    .text_color(color)
                    .hint_text(tl!("Type text"))
                    .id(input)
                    .show(ui);
                let resp = output.response;
                if let Some(cursor) = output.cursor_range {
                    let chars = cursor.as_sorted_char_range();
                    let byte = |index: usize| t.text.char_indices().nth(index).map_or(t.text.len(), |(at, _)| at);
                    t.selection = Some(byte(chars.start.into())..byte(chars.end.into()));
                }
                // Each paragraph keeps its own alignment through the typing.
                if t.text != before {
                    crate::edit_text_ui::follow_typing(ui.ctx(), input, &before, &t.text, &mut [&mut t.style.aligns]);
                }
                // The editor has no visible label; name it for assistive tech.
                resp.response.clone().accessible_name(tl!("Added text"));
                if resp.secondary_clicked() && !t.has_unapplied_typing() {
                    menu_at = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.hover_pos()));
                }
                if t.focus {
                    resp.request_focus();
                    if let Some(range) = &t.selection {
                        crate::edit_text_ui::reselect(ui.ctx(), input, &t.text, range);
                    }
                    t.focus = false;
                }
                let buttons = egui::Frame::NONE
                    .fill(tokens.card)
                    .stroke(Stroke::new(1.0, tokens.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(2)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                        ui.horizontal(|ui| {
                            done = crate::icons::button(ui, "check", 24.0, false, tl!("Done adding text")).clicked();
                            cancel = crate::icons::button(ui, "x", 24.0, false, tl!("Discard this text (Esc)")).clicked();
                        });
                    })
                    .response
                    .rect;
                // Pressing a button, or moving the keyboard to one, takes the focus from the text:
                // that is not a click away.
                let on_buttons = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| buttons.contains(p))
                    || crate::edit_text_ui::focus_pos(ui.ctx()).is_some_and(|p| buttons.contains(p));
                cancel |= ui.input(|i| i.key_pressed(egui::Key::Escape));
                // Ctrl+Enter (⌘Enter) is Done, as it applies an edit in Edit text (Thai edition).
                done |= ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
                commit = done || resp.lost_focus() && !on_buttons && !outside && !view.objects.hold_editor;
            });
            // Page text is drawn with the standard fonts; anything else would be saved as `?` (#125).
            undrawable = self::undrawable(t, memo);
            if let Some(c) = undrawable {
                ui.label(egui::RichText::new(undrawable_message(c)).small().color(UNDRAWABLE));
            }
        });
    if cancel {
        view.content.draft = None;
        return (None, false);
    }
    // Right-clicked, unchanged: the box closes and the page's menu opens for it as for a box
    // selected with a click (Thai edition: Split into lines and Arrange only showed after a move).
    if let Some(pos) = menu_at
        && let Some(d) = view.content.draft.as_ref()
        && let Some(i) = d.index
        && on_page(added, d.page).into_iter().any(|(j, a)| j == i && matches!(&a.content, AddedContent::Text(t) if *t == d.style))
    {
        let page = d.page;
        view.content.draft = None;
        view.content.selected = Some((page, i));
        view.content.menu = Some((page, i));
        view.content.menu_at = Some(pos);
        return (None, false);
    }
    // Text that can't be drawn stays in the editor, with the warning, rather than being lost.
    if !commit || undrawable.is_some() {
        return (None, false);
    }
    let edit = view.content.draft.take().and_then(|d| finish(&mut view.content, d, added, info));
    (edit, done)
}

/// A finished draft as an edit: new text is added (and selected once it exists), retyped text
/// updated, and text emptied by retyping removed. Empty new text adds nothing.
fn finish(cv: &mut ContentView, d: TextDraft, added: &[Added], info: &DocInfo) -> Option<Edit> {
    // Unchanged is no edit, even for text saved with trailing spaces the trim would drop.
    if let Some(i) = d.index
        && let Some((_, a)) = on_page(added, d.page).into_iter().find(|(k, _)| *k == i)
        && let AddedContent::Text(old) = &a.content
        && (*old == (AddedText { text: d.text.clone(), rect: d.rect, ..d.style.clone() })
            // A box that hugs its text sizes itself as it opens: that is no change either.
            || d.auto && *old == (AddedText { text: d.text.clone(), rect: old.rect, ..d.style.clone() }))
    {
        return None;
    }
    let text = d.text.trim_end().to_string();
    let aligns = pdfcraft_engine::paragraph_align::remap(&d.text, &text, &d.style.aligns);
    let d = TextDraft { style: AddedText { aligns, ..d.style }, ..d };
    // A box that hugs its text is refitted to what was typed.
    let fit = |t: AddedText| match info.pages.get(d.page) {
        Some(p) if d.auto || t.direction != pdfcraft_engine::TextDirection::Horizontal => pdfcraft_engine::fitted_text(&t, p.width as f64),
        _ => t,
    };
    match d.index {
        None if text.trim().is_empty() => None,
        None => {
            cv.select_added = Some((d.page, on_page(added, d.page).len()));
            Some(Edit::AddText { page: d.page, text: fit(AddedText { rect: d.rect, text, ..d.style }) })
        }
        Some(i) if text.trim().is_empty() => Some(Edit::DeleteContent { page: d.page, index: i }),
        Some(i) => {
            let (_, a) = on_page(added, d.page).into_iter().find(|(k, _)| *k == i)?;
            let AddedContent::Text(old) = &a.content else { return None };
            let updated = fit(AddedText { text, rect: d.rect, ..d.style });
            (*old != updated).then_some(Edit::UpdateContent { page: d.page, index: i, content: AddedContent::Text(updated) })
        }
    }
}

/// A restyled item: one whose box hugged its text is refitted to the new font or size. A new
/// size (Thai edition) grows or shrinks the box about its centre, so the text stays where it was.
pub(crate) fn restyled(old: &AddedText, new: AddedText, page_width: Option<f64>) -> AddedText {
    let resized = new.size != old.size && new.direction == old.direction;
    let out = restyled_at(old, new, page_width);
    if !resized {
        return out;
    }
    let (a, b) = (pdfcraft_engine::added_text_rect(old), pdfcraft_engine::added_text_rect(&out));
    let dx = (a[0] + a[2] - b[0] - b[2]) / 2.0;
    let dy = (a[1] + a[3] - b[1] - b[3]) / 2.0;
    if !(dx.is_finite() && dy.is_finite()) {
        return out;
    }
    let r = out.rect;
    AddedText { rect: [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy], ..out }
}

fn restyled_at(old: &AddedText, new: AddedText, page_width: Option<f64>) -> AddedText {
    match page_width {
        // A box turned another way (Thai edition) hugs its text again, from its top left corner.
        Some(w) if new.direction != old.direction => {
            let r = pdfcraft_engine::added_text_rect(old);
            pdfcraft_engine::fitted_text(&AddedText { rect: [r[0], r[3] - new.size, r[0] + w.max(1.0), r[3]], ..new }, w)
        }
        // Aligning moves the text inside its box, which keeps its size (Thai edition, the owner's
        // choice): refitting would hug the text again and leave nothing to align it in.
        _ if only_alignment_changed(old, &new) => new,
        // Only a box that hugs its text follows it; one made wider by hand keeps its width.
        Some(w) if pdfcraft_engine::hugs_text(old, w) => pdfcraft_engine::fitted_text(&new, w),
        _ => new,
    }
}

/// Whether `new` differs from `old` in its alignment only.
fn only_alignment_changed(old: &AddedText, new: &AddedText) -> bool {
    (new.align != old.align || new.aligns != old.aligns) && AddedText { align: old.align, aligns: old.aligns.clone(), ..new.clone() } == *old
}

/// The Add text font menu: the bundled families, then the fonts installed on this machine, with
/// a box to search them by name.
pub(crate) fn family_menu(ui: &mut egui::Ui, family: &mut FontFamily) {
    let search_id = ui.make_persistent_id("font-family-search");
    craft_ui::ComboBox::from_id_salt("font-family")
        .selected_text(family.label())
        .width(130.0)
        .height(360.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            if let Some(f) = family_list(ui, search_id, Some(*family), &mut None) {
                *family = f;
            }
        });
}

/// The font of a text comment or Fill & Sign text (Thai edition): the font menu, then Bold and
/// Italic. True when one changed.
pub(crate) fn text_font_controls(ui: &mut egui::Ui, font: &mut pdfcraft_engine::TextFont, box_size: f32) -> bool {
    let before = font.clone();
    // A family this machine doesn't have shows as Helvetica, and stays as it is until another
    // is picked.
    let old = FontFamily::from_name(font.family_label()).unwrap_or_default();
    let mut family = old;
    family_menu(ui, &mut family);
    if family != old {
        font.family = if family == FontFamily::Helvetica { String::new() } else { family.label().to_string() };
    }
    if crate::icons::button(ui, "bold", box_size, font.bold, tl!("Bold")).clicked() {
        font.bold = !font.bold;
    }
    if crate::icons::button(ui, "italic", box_size, font.italic, tl!("Italic")).clicked() {
        font.italic = !font.italic;
    }
    *font != before
}

/// [`text_font_controls`] for a tool rail (Thai edition): a button that opens Bold, Italic and
/// the font list beside the rail. True when one changed.
pub(crate) fn text_font_rail(ui: &mut egui::Ui, font: &mut pdfcraft_engine::TextFont) -> bool {
    let before = font.clone();
    let tip = format!("{}: {}", tl!("Font"), font.family_label());
    let resp = crate::icons::rail_button(ui, "type", 32.0, false, &tip);
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.horizontal(|ui| {
            if crate::icons::button(ui, "bold", 24.0, font.bold, tl!("Bold")).clicked() {
                font.bold = !font.bold;
            }
            if crate::icons::button(ui, "italic", 24.0, font.italic, tl!("Italic")).clicked() {
                font.italic = !font.italic;
            }
        });
        let current = FontFamily::from_name(font.family_label());
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            if let Some(f) = family_list(ui, ui.make_persistent_id("rail-font-search"), current, &mut None) {
                font.family = if f == FontFamily::Helvetica { String::new() } else { f.label().to_string() };
                ui.close();
            }
        });
    });
    *font != before
}

/// The inside of a font menu (Thai edition): a search box, the bundled families, then the
/// families installed on this machine, Thai ones marked TH. Returns the family picked (a click,
/// or Enter on the first match), which also closes the menu; `current` is ticked and `hovered`
/// gets the family under the pointer.
pub(crate) fn family_list(
    ui: &mut egui::Ui,
    search_id: egui::Id,
    current: Option<FontFamily>,
    hovered: &mut Option<FontFamily>,
) -> Option<FontFamily> {
    ui.set_min_width(240.0);
    // Opening the menu starts a new search with the caret in the box, so the font's name
    // can be typed straight away.
    let now = ui.input(|i| i.time);
    let seen_id = search_id.with("seen");
    let opened = ui.data(|d| d.get_temp::<f64>(seen_id)).is_none_or(|seen| now - seen > 0.3);
    ui.data_mut(|d| d.insert_temp(seen_id, now));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    let mut query: String = if opened { String::new() } else { ui.data(|d| d.get_temp(search_id)).unwrap_or_default() };
    let search = ui.add(egui::TextEdit::singleline(&mut query).hint_text(tl!("Search")).desired_width(f32::INFINITY));
    if opened {
        search.request_focus();
    }
    // Enter takes the first font the search leaves.
    let entered = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let want = query.trim().to_lowercase();
    ui.data_mut(|d| d.insert_temp(search_id, query));
    let mut picked = None;
    let mut first = None;
    let mark = ui.visuals().weak_text_color();
    let mut offer = |ui: &mut egui::Ui, f: FontFamily| {
        if !(want.is_empty() || f.label().to_lowercase().contains(&want)) {
            return;
        }
        first.get_or_insert(f);
        let resp = ui.selectable_label(current == Some(f), f.label());
        // A small mark on the fonts that have Thai letters of their own.
        if f.has_thai() {
            let at = Pos2::new(ui.max_rect().right() - 6.0, resp.rect.center().y);
            ui.painter().text(at, egui::Align2::RIGHT_CENTER, "TH", egui::FontId::proportional(9.0), mark);
        }
        if resp.hovered() {
            *hovered = Some(f);
        }
        if resp.clicked() {
            picked = Some(f);
            ui.close();
        }
    };
    for f in FontFamily::bundled() {
        offer(ui, f);
    }
    ui.separator();
    // Names that start with what was typed come before those that only contain it.
    let (starts, rest): (Vec<FontFamily>, Vec<FontFamily>) =
        FontFamily::installed().into_iter().partition(|f| want.is_empty() || f.label().to_lowercase().starts_with(&want));
    for f in starts.into_iter().chain(rest) {
        offer(ui, f);
    }
    if entered && let Some(f) = first.filter(|_| !want.is_empty()) {
        picked = Some(f);
        ui.close();
    }
    picked
}

/// How long the wheel rests on the size box before the size it reached is applied.
const WHEEL_REST: f64 = 0.7;

/// The text size: a box to type any size from 1 to 500 points into (Enter, or a click away,
/// applies it), with the usual sizes in a list beside it. Each applied size is one undo step.
/// `hovered` gets the listed size under the pointer. The wheel over them steps the size a point at
/// a time, shown as a preview (`hovered` again) and applied, as one step, once the wheel rests.
pub(crate) fn size_control(ui: &mut egui::Ui, size: &mut f64, hovered: &mut Option<f64>, mixed: bool) -> bool {
    let before = *size;
    let shown = |v: f64| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
    let id = ui.make_persistent_id("font-size-typed");
    let mut committed = false;
    // What is in the box, and the size it was typed over. The text is kept while the box has the
    // caret (also across the window losing focus) and follows the size when that changes
    // elsewhere: a preset, or another item selected.
    let kept: Option<(String, f64)> = ui.data(|d| d.get_temp(id));
    let wheel_id = id.with("wheel");
    let mut wheel: Option<(f64, f64)> = ui.data(|d| d.get_temp(wheel_id));
    let mut typed = match wheel {
        Some((value, _)) => shown(value),
        None => kept.filter(|(_, over)| *over == *size).map_or_else(|| if mixed { tl!("Mixed").to_owned() } else { shown(*size) }, |(text, _)| text),
    };
    // Narrow, with no unit beside it, so the Format text row keeps its width (the tip says what it is):
    // as wide as the largest size, three digits, in the box's own font, with its margins (the width
    // given is the whole box's) and a point for the caret.
    const MARGIN: i8 = 4;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let digits =
        ui.fonts_mut(|f| f.layout_no_wrap(if mixed { tl!("Mixed").to_owned() } else { "500".to_owned() }, font, egui::Color32::BLACK).size().x);
    let resp = ui
        .add(
            egui::TextEdit::singleline(&mut typed)
                .margin(egui::Margin::symmetric(MARGIN, 2))
                .desired_width(digits + 2.0 * f32::from(MARGIN) + 2.0)
                .align(egui::Align2::RIGHT_CENTER),
        )
        .tip(tl!("Font size"));
    // Clicking into the box selects the size, so typing replaces it.
    if resp.gained_focus()
        && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id)
    {
        let end = egui::text::CCursor::new(typed.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
        state.store(ui.ctx(), resp.id);
    }
    if resp.lost_focus() {
        if let Ok(v) = typed.trim().replace(',', ".").parse::<f64>()
            && v.is_finite()
        {
            *size = (v.clamp(1.0, 500.0) * 10.0).round() / 10.0;
            committed = mixed || *size != before;
        }
        ui.data_mut(|d| d.remove::<(String, f64)>(id));
    } else {
        ui.data_mut(|d| d.insert_temp(id, (typed, *size)));
    }
    let presets = craft_ui::ComboBox::from_id_salt("font-size").selected_text("").width(16.0).show_ui(ui, |ui| {
        for preset in [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0, 48.0, 72.0] {
            if ui.selectable_value(size, preset, format!("{preset} pt")).hovered() {
                *hovered = Some(preset);
            }
        }
    });
    let over = resp.hovered() || presets.response.hovered();
    // An arrow alone: the tip names it, for the pointer and for assistive tech.
    presets.response.tip(tl!("Common font sizes"));
    let notches: f64 = if over {
        let step = |e: &egui::Event| match e {
            egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => f64::from(delta.y.signum()),
            _ => 0.0,
        };
        ui.input(|i| i.events.iter().map(step).sum())
    } else {
        0.0
    };
    let now = ui.input(|i| i.time);
    if notches != 0.0 {
        let from = wheel.map_or(*size, |(value, _)| value);
        wheel = Some(((from.round() + notches).clamp(1.0, 500.0), now));
        // The panel doesn't scroll under the wheel as well.
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    }
    match wheel {
        Some((value, at)) if now - at >= WHEEL_REST => {
            *size = value;
            committed = true;
            ui.data_mut(|d| d.remove::<(f64, f64)>(wheel_id));
            // A box with the caret shows what was applied, not what the wheel left in it.
            ui.data_mut(|d| d.remove::<(String, f64)>(id));
        }
        Some(pending) => {
            *hovered = Some(pending.0);
            ui.data_mut(|d| d.insert_temp(wheel_id, pending));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        None => {}
    }
    committed || *size != before
}

/// What the pointer rests on in Format text, shown on the selected text or in the open editor
/// before it is picked (Thai edition): a font, a size or a colour from their lists, or bold,
/// italic or underline turned the other way. Nothing is applied until it is clicked.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LookPreview {
    pub family: Option<FontFamily>,
    pub size: Option<f64>,
    pub color: Option<[f64; 3]>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    /// Which way the text runs (Thai edition); a paragraph of the page turned becomes an added box.
    pub direction: Option<pdfcraft_engine::TextDirection>,
    /// The alignment of the whole box (Thai edition): its text moves inside the box, which keeps
    /// its size.
    pub align: Option<TextAlign>,
}

impl LookPreview {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `look` as previewed.
    pub fn apply(&self, look: &mut AddedText) {
        if let Some(f) = self.family {
            look.family = f;
        }
        if let Some(v) = self.size {
            look.size = v;
        }
        if let Some(c) = self.color {
            look.color = c;
        }
        if let Some(b) = self.bold {
            look.bold = b;
        }
        if let Some(i) = self.italic {
            look.italic = i;
        }
        if let Some(u) = self.underline {
            look.underline = u;
        }
        if let Some(d) = self.direction {
            look.direction = d;
        }
        if let Some(a) = self.align {
            look.align = a;
            look.aligns.clear();
        }
    }
}

/// The preview under the pointer: (what, when the pointer reached it, when it was last seen there).
type LookHover = (LookPreview, f64, f64);

fn hover_id() -> egui::Id {
    egui::Id::unique("format-text-look-hover")
}

/// How long the pointer rests on a choice before the text previews it (a sweep down a list loads
/// nothing), and how long a preview outlives the last frame that saw the pointer there.
const HOVER_DWELL: f64 = 0.2;
const HOVER_FRESH: f64 = 0.25;

/// Format text tells, each frame it is drawn, what the pointer rests on (`None`: nothing).
pub(crate) fn note_hover(ctx: &egui::Context, hovered: Option<LookPreview>) {
    let now = ctx.input(|i| i.time);
    let before: Option<LookHover> = ctx.data(|d| d.get_temp(hover_id()));
    match hovered.filter(|h| !h.is_empty()) {
        Some(h) => {
            let since = before.filter(|(was, ..)| *was == h).map_or(now, |(_, since, _)| since);
            ctx.data_mut(|d| d.insert_temp::<LookHover>(hover_id(), (h, since, now)));
            // Wake up for the preview to start, and afterwards to notice the pointer leaving.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        None if before.is_some() => {
            ctx.data_mut(|d| d.remove::<LookHover>(hover_id()));
            ctx.request_repaint();
        }
        None => {}
    }
}

/// The look the pointer rests on in Format text, to preview on the selected text (Add text) or in
/// the open editor (Edit text).
pub(crate) fn previewed_look(ctx: &egui::Context) -> Option<LookPreview> {
    let now = ctx.input(|i| i.time);
    let (look, since, seen): LookHover = ctx.data(|d| d.get_temp(hover_id()))?;
    if now - seen > HOVER_FRESH {
        // The panel or a menu closed with the pointer on a choice.
        ctx.data_mut(|d| d.remove::<LookHover>(hover_id()));
        ctx.request_repaint();
        return None;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    (now - since >= HOVER_DWELL).then_some(look)
}

/// The selected text as it would look with what the pointer rests on in Format text, drawn over
/// the page until the pointer moves on. Nothing is changed in the document.
/// Returns the box the previewed text takes, for the selection frame to follow.
pub(crate) fn paint_look_preview(ui: &egui::Ui, painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, t: &AddedText) -> Option<Rect> {
    let preview = previewed_look(ui.ctx())?;
    let mut look = t.clone();
    preview.apply(&mut look);
    if look == *t {
        return None;
    }
    let page_width = info.pages.get(page).map(|p| p.width as f64);
    let shown = restyled(t, look.clone(), page_width);
    let (old, new) = (screen_rect(xf, info, page, t.rect), screen_rect(xf, info, page, shown.rect));
    let zoom = xf.rect.width() / xf.pw.max(1.0);
    let font = crate::theme::document_font(ui.ctx(), look.family, look.bold, look.italic, (look.size as f32 * zoom).max(4.0));
    let [cr, cg, cb] = look.color.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
    let color = Color32::from_rgb(cr, cg, cb);
    let galley = painter.layout(t.text.clone(), font, color, new.width().max(8.0));
    let x = match t.align {
        TextAlign::Left | TextAlign::Justify => new.left(),
        TextAlign::Center => new.center().x - galley.size().x / 2.0,
        TextAlign::Right => new.right() - galley.size().x,
    };
    let text = Rect::from_min_size(Pos2::new(x, new.top()), galley.size());
    painter.rect_filled(old.union(new).union(text).expand(2.0), CornerRadius::ZERO, Color32::WHITE);
    painter.galley(text.min, galley, color);
    Some(new.union(text))
}

/// The added-text box's text field.
pub(crate) fn input_id() -> egui::Id {
    egui::Id::unique("added-text-input")
}

/// `text` laid out as added text with `style` is drawn: each paragraph (line typed with Enter) wrapped
/// to `wrap` and placed by its own alignment, a justified one with every row but its last
/// stretched. With `hugs` (Thai edition) a paragraph doesn't wrap: `wrap` is only the width it is
/// aligned in.
#[allow(clippy::too_many_arguments)]
fn aligned_galley(
    ui: &egui::Ui,
    text: &str,
    style: &AddedText,
    font: &egui::FontId,
    color: Color32,
    letter_spacing: f32,
    wrap: f32,
    hugs: bool,
) -> std::sync::Arc<egui::Galley> {
    use pdfcraft_engine::TextAlign;
    let line_width = if hugs { f32::INFINITY } else { wrap };
    // Lines as far apart as the page sets them, when the spacing isn't the ordinary one.
    let line_height = (style.line_spacing != 0.0).then(|| font.size * (style.line_advance() / style.size.max(f64::EPSILON)) as f32);
    let job = |text: &str, align: TextAlign| {
        let mut job = spaced_job(text, font, color, letter_spacing, style.underline, line_width, egui::Align::Min);
        job.justify = align == TextAlign::Justify && !hugs;
        for s in &mut job.sections {
            s.format.line_height = line_height;
        }
        job
    };
    let uniform = text.split('\n').enumerate().all(|(i, _)| style.paragraph_align(i) == style.align);
    if uniform && style.align != TextAlign::Center && style.align != TextAlign::Right {
        return ui.fonts_mut(|f| f.layout_job(job(text, style.align)));
    }
    // Each paragraph on its own (egui aligns a whole job one way), its rows moved within the
    // box's width, then joined in order: the caret and selection see one text.
    let mut galleys = Vec::new();
    let mut start = 0;
    for (i, paragraph) in text.split('\n').enumerate() {
        let end = start + paragraph.len();
        // The last paragraph keeps a trailing break, as egui's own split does.
        let last = end + 1 == text.len();
        let piece = if last { text.get(start..).unwrap_or_default() } else { paragraph };
        let mut galley = ui.fonts_mut(|f| f.layout_job(job(piece, style.paragraph_align(i))));
        let g = std::sync::Arc::make_mut(&mut galley);
        for placed in &mut g.rows {
            let free = (wrap - placed.row.size.x).max(0.0);
            placed.pos.x = match style.paragraph_align(i) {
                TextAlign::Center => free / 2.0,
                TextAlign::Right => free,
                TextAlign::Left | TextAlign::Justify => 0.0,
            };
        }
        g.rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(g.rect.width().max(wrap), g.rect.height()));
        galleys.push(galley);
        if last {
            break;
        }
        start = end + 1;
    }
    let mut whole = spaced_job(text, font, color, letter_spacing, style.underline, line_width, egui::Align::Min);
    for s in &mut whole.sections {
        s.format.line_height = line_height;
    }
    let whole = std::sync::Arc::new(whole);
    std::sync::Arc::new(egui::Galley::concat(whole, &galleys, ui.ctx().pixels_per_point()))
}

/// How to use the tools, under the Add content list.
pub(crate) fn hint(ui: &mut egui::Ui, t: &Tokens) {
    ui.label(
        egui::RichText::new(
            tl!("Click on the page to add text; each click starts a new box, and ✓ finishes. Drag items to move them, drag a corner to resize, double-click text to edit it."),
        )
            .small()
            .color(t.text_faint),
    );
    ui.add_space(4.0);
}

impl crate::PdfCraftApp {
    /// Pick an image file for the active document, read it, and call `then` with its name and
    /// bytes: now when `save_override` answers (tests and automation never see a native dialog),
    /// otherwise on a later frame, and only if that document is still the active one.
    #[cfg(not(target_arch = "wasm32"))]
    fn pick_image(&mut self, title: &str, then: impl FnOnce(&mut Self, String, Vec<u8>) + Send + 'static) {
        let read = |app: &mut Self, path: std::path::PathBuf| match std::fs::read(&path) {
            Ok(bytes) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                then(app, name, bytes);
            }
            Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
        };
        match self.save_override.clone() {
            Some(p) if p.ends_with(".png") || p.ends_with(".jpg") => read(self, p.into()),
            Some(_) => {}
            None => {
                let dialog = rfd::AsyncFileDialog::new()
                    .add_filter_any_case(tl!("Images"), &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"])
                    .set_title(title);
                let target = self.active_ids().map(|(_, id)| id);
                self.ask_one(crate::pickers::Ask::File(dialog), target, read);
            }
        }
    }

    /// Edit a PDF ▸ Add content ▸ Image: pick a file and place it in the middle of the current page.
    pub fn add_image_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Choose an image"), |app, name, bytes| app.add_image(name, bytes));
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Adding images arrives on the web with file pickers for images");
    }

    /// Edit text & images ▸ Replace Image: pick a file to draw in a page image's place.
    pub(crate) fn replace_page_image_dialog(&mut self, page: usize, index: usize) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Replace image"), move |app, name, bytes| {
            app.apply_edit(Edit::EditPageImage {
                page,
                index,
                change: pdfcraft_engine::ImageEdit::Replace { name, bytes: std::sync::Arc::new(bytes) },
            });
        });
        #[cfg(target_arch = "wasm32")]
        self.notify_fmt(
            "Replacing images on page {p} arrives on the web with image pickers ({i})",
            &[("p", &(page + 1).to_string()), ("i", &index.to_string())],
        );
    }

    /// Edit text & images ▸ Save Image As.
    pub(crate) fn save_page_image(&mut self, page: usize, index: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let file = self.session.get(id).map(|d| d.page_image_file(page, index));
        match file {
            Some(Ok((ext, bytes))) => {
                self.write_files(&[(format!("Image page {} #{}.{ext}", page + 1, index + 1), std::sync::Arc::new(bytes))], "Save image");
            }
            Some(Err(e)) => self.notify_fmt("Couldn't save the image: {e}", &[("e", &e.to_string())]),
            None => {}
        }
    }

    /// Copy Image: image `index` on `page`, at its own resolution, to the clipboard.
    pub fn copy_page_image(&mut self, page: usize, index: usize) -> bool {
        // A newer copy wins over a whole-document copy still reading its pages.
        self.document_copy = None;
        let Some((_, id)) = self.active_ids() else { return false };
        if !self.may_copy(id) {
            return false;
        }
        let Some(pixels) = self.session.get(id).map(|d| d.page_image_rgba(page, index)) else { return false };
        let copied = pixels.and_then(|(w, h, rgba)| {
            #[cfg(not(target_arch = "wasm32"))]
            if self.system_clipboard {
                let img = arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Borrowed(&rgba) };
                crate::os_clipboard::with(|c| c.set_image(img)).map_err(|e| e.to_string())?;
            }
            #[cfg(target_arch = "wasm32")]
            if self.system_clipboard {
                return Err("the web version can't put images on the clipboard yet; use Save Image As".to_string());
            }
            self.last_copied_file = None;
            self.last_copied_image = Some((w, h, rgba));
            Ok((w, h))
        });
        self.page_image_clip = copied.is_ok().then(|| self.page_image_clip_of(id, page, index)).flatten();
        match copied {
            Ok((w, h)) => {
                self.notify_fmt("The image has been copied ({w} × {h} pixels)", &[("w", &w.to_string()), ("h", &h.to_string())]);
                true
            }
            Err(e) => {
                self.notify_fmt("Couldn't copy the image: {e}", &[("e", &e)]);
                false
            }
        }
    }

    /// What to remember of the page's picture `index` on `page` when it is copied: its whole box
    /// in display space (Thai edition).
    fn page_image_clip_of(&self, doc: pdfcraft_engine::DocId, page: usize, index: usize) -> Option<crate::state::PageImageClip> {
        let d = self.session.get(doc)?;
        let image = d.page_images(page).into_iter().nth(index)?;
        let info = d.info.pages.get(page)?;
        let [x0, y0, x1, y1] = image.rect;
        let corners = [(x0, y0), (x1, y1), (x0, y1), (x1, y0)].map(|(x, y)| {
            let v = info.user_to_view(x as f32, y as f32);
            [f64::from(v[0]), f64::from(info.height) - f64::from(v[1])]
        });
        let xs = corners.map(|c| c[0]);
        let ys = corners.map(|c| c[1]);
        let rect = [
            xs.iter().copied().fold(f64::INFINITY, f64::min),
            ys.iter().copied().fold(f64::INFINITY, f64::min),
            xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ];
        (rect.iter().all(|v| v.is_finite()) && rect[2] > rect[0] && rect[3] > rect[1]).then_some(crate::state::PageImageClip {
            doc,
            page,
            index,
            object: image.object,
            rect,
            pastes: 0,
        })
    }

    /// Paste the copied page picture (Thai edition) onto `page`: the same picture at the same size,
    /// a step down and right on its own page, where it was on another, or centred at `at`. While
    /// the picture is still there it is copied as it is (its image object, as Ctrl+drag does);
    /// after a cut, or into another document, its pixels go in at its size. `false` when there is
    /// nothing of it to paste (the caller pastes the pixels as before).
    fn paste_page_image_clip(&mut self, id: pdfcraft_engine::DocId, page: usize, at: Option<[f64; 2]>, bytes: &[u8], name: &str) -> bool {
        let Some(clip) = self.page_image_clip.clone() else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let Some(info) = doc.info.pages.get(page) else { return false };
        let [x0, y0, x1, y1] = clip.rect;
        let (w, h) = (x1 - x0, y1 - y0);
        let rect = if let Some(at) = at {
            let v = info.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
            let c = [v[0], f64::from(info.height) - v[1]];
            [c[0] - w / 2.0, c[1] - h / 2.0, c[0] + w / 2.0, c[1] + h / 2.0]
        } else {
            let same_page = clip.doc == id && clip.page == page;
            let steps = if same_page { clip.pastes.saturating_add(1) } else { clip.pastes };
            let shift = crate::state::PASTE_STEP * f64::from(steps.min(40));
            [x0 + shift, y0 - shift, x1 + shift, y1 - shift]
        };
        if !rect.iter().all(|v| v.is_finite()) {
            return false;
        }
        let still_there = clip.doc == id
            && clip.object.is_some()
            && doc.page_images(clip.page).into_iter().nth(clip.index).is_some_and(|im| im.object == clip.object);
        let edit = if still_there {
            Edit::MovePageImageToPage { page: clip.page, index: clip.index, to_page: page, rect, copy: true }
        } else {
            Edit::AddImage { page, rect: Some(rect), name: name.to_string(), bytes: std::sync::Arc::new(bytes.to_vec()) }
        };
        let Some((i, _)) = self.active_ids() else { return false };
        if self.paste_image_edit(i, id, page, edit)
            && let Some(c) = self.page_image_clip.as_mut()
        {
            c.pastes = c.pastes.saturating_add(1);
        }
        true
    }

    /// The picture's visible box on its page, in user space.
    pub(crate) fn page_image_box(&self, page: usize, index: usize) -> Option<[f64; 4]> {
        let (_, id) = self.active_ids()?;
        let image = self.session.get(id)?.page_images(page).into_iter().nth(index)?;
        Some(image.visible_rect.unwrap_or(image.rect))
    }

    /// Cut: copy the picture, and only when that worked take it off the page (one undo step).
    pub(crate) fn cut_page_image(&mut self, page: usize, index: usize) -> bool {
        let Some((view, id)) = self.active_ids() else { return false };
        if self.session.get(id).is_some_and(|doc| !doc.allows_modification()) {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }
        if !self.copy_page_image(page, index) || !self.apply_edit(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Delete }) {
            return false;
        }
        if let Some(view) = self.views.get_mut(view) {
            view.image_selection = None;
        }
        true
    }

    /// Redact image: a redaction mark over the whole picture. It is applied, like any mark, from
    /// Redact a PDF.
    pub(crate) fn redact_page_image(&mut self, page: usize, index: usize) {
        let Some(rect) = self.page_image_box(page, index) else { return };
        let author = self.comment_prefs.author.clone();
        let mark = self.redact_prefs.mark(page, vec![pdfcraft_engine::rect_quad(rect)], &author);
        if self.apply_edit(mark) {
            self.notify_tr("The image is marked for redaction. Apply it with Redact a PDF");
        }
    }

    /// Create link: the link properties for a new link over the picture.
    pub(crate) fn link_page_image(&mut self, page: usize, index: usize) {
        if let Some(rect) = self.page_image_box(page, index) {
            self.open_link_props(page, Some(rect), None);
        }
    }

    /// Recognize text: the Recognize Text dialog, set to this page.
    pub fn recognize_text_on_page(&mut self, page: usize) {
        self.ocr_draft.pages = crate::ocr_ui::OcrPages::Range;
        self.ocr_draft.from = page + 1;
        self.ocr_draft.to = page + 1;
        self.dialog = Some(crate::Dialog::RecognizeText);
    }

    /// Click an image field: pick a picture for it.
    pub fn choose_field_image(&mut self, name: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let name = name.to_string();
            self.pick_image(tl!("Select Icon"), move |app, _, bytes| {
                app.apply_edit(Edit::SetFieldImage { name, image: std::sync::Arc::new(bytes) });
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_fmt("{name}: choosing images arrives on the web with file pickers for images", &[("name", &name)]);
    }

    /// Edit image ▸ Replace: pick a file for the selected image.
    pub fn replace_image_dialog(&mut self, page: usize, index: usize) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_image(tl!("Replace image"), move |app, name, bytes| {
            app.apply_edit(Edit::ReplaceImage { page, index, name, bytes: std::sync::Arc::new(bytes) });
        });
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (page, index);
            self.notify_tr("Replacing images arrives on the web with image file pickers");
        }
    }

    /// Place image `bytes` in the middle of the current page and select it.
    pub fn add_image(&mut self, name: String, bytes: Vec<u8>) {
        let Some((i, id)) = self.active_ids() else { return };
        let page = self.views[i].current;
        let before_img = self.session.get(id).map_or(0, |d| d.page_images(page).len());
        let before_added = self.session.get(id).map_or(0, |d| d.added().iter().filter(|a| a.page == page).count());
        if self.apply_edit(Edit::AddImage { page, rect: None, name, bytes: std::sync::Arc::new(bytes) }) {
            self.views[i].content.selected = Some((page, before_added));
            self.views[i].content.select_added = Some((page, before_added));
            self.views[i].image_selection = Some(crate::edit_text_ui::ImageSelection::at(page, before_img));
            self.quick_tool = crate::QuickTool::EditText;
            self.left = crate::LeftPanel::Tool("edit");
            self.left_open = true;
        }
    }

    /// Whether there is an image in the in-memory clipboard or system clipboard ready to paste.
    pub fn can_paste_image(&self) -> bool {
        if let Some(crate::create_ui::Clip::Image { .. }) = self.clipboard_override.as_ref() {
            return true;
        }
        if self.clipboard_override.is_some() {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.system_clipboard {
                return matches!(crate::os_clipboard::for_image_paste(), Some(crate::create_ui::Clip::Image { .. }));
            }
        }
        self.last_copied_image.is_some() || self.last_copied_file.is_some()
    }

    /// Paste the integration's delivered pixels without rereading a clipboard that may have changed.
    pub(crate) fn paste_received_image(&mut self, page: usize, image: &egui::ColorImage) -> bool {
        let [width, height] = image.size;
        let pixels = width.checked_mul(height);
        let bytes = pixels.and_then(|n| n.checked_mul(4));
        if width == 0
            || height == 0
            || width > crate::create_ui::MAX_IMAGE_SIDE
            || height > crate::create_ui::MAX_IMAGE_SIDE
            || pixels != Some(image.pixels.len())
            || bytes.is_none_or(|n| n > crate::create_ui::MAX_IMAGE_BYTES)
        {
            self.notify_error("the clipboard image is empty, malformed or exceeds size limits");
            return false;
        }
        let rgba = image.pixels.iter().flat_map(egui::Color32::to_srgba_unmultiplied).collect();
        let delivered = crate::create_ui::Clip::Image { width, height, rgba };
        self.paste_image_with_system(page, None, None, None, false, Some(delivered))
    }

    /// Paste an image from the clipboard onto `page`.
    pub fn paste_image(&mut self, page: usize, rect: Option<[f64; 4]>) -> bool {
        self.paste_image_internal(page, rect, None)
    }

    /// Paste an image centered at context menu point `at: [f64; 2]` on `page`.
    pub fn paste_image_at(&mut self, page: usize, at: [f64; 2]) -> bool {
        self.paste_image_internal(page, None, Some(at))
    }

    fn paste_image_internal(&mut self, page: usize, rect: Option<[f64; 4]>, at: Option<[f64; 2]>) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        let system = if self.clipboard_override.is_none() && self.system_clipboard { crate::os_clipboard::for_image_paste() } else { None };
        #[cfg(target_arch = "wasm32")]
        let system = None;
        self.paste_image_with_system(
            page,
            rect,
            at,
            system,
            cfg!(not(target_arch = "wasm32")) && self.system_clipboard,
            self.clipboard_override.clone(),
        )
    }

    fn paste_image_with_system(
        &mut self,
        page: usize,
        rect: Option<[f64; 4]>,
        at: Option<[f64; 2]>,
        system: Option<crate::create_ui::Clip>,
        system_authoritative: bool,
        clipboard: Option<crate::create_ui::Clip>,
    ) -> bool {
        let Some((i, id)) = self.active_ids() else {
            return false;
        };
        if let Some(doc) = self.session.get(id)
            && !doc.allows_modification()
        {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }

        // Whether what is pasted is the picture last copied here (Thai edition: a page picture copied
        // pastes at its size, `page_image_clip`).
        let mut copied_here = false;
        let image_data: Option<(String, Vec<u8>)> = match clipboard {
            Some(crate::create_ui::Clip::Image { width, height, rgba }) => {
                let is_same = self
                    .last_copied_image
                    .as_ref()
                    .is_some_and(|(w, h, bytes)| *w as usize == width && *h as usize == height && bytes.as_slice() == rgba.as_slice());
                copied_here = is_same && self.last_copied_file.is_none();
                if !is_same {
                    self.last_copied_file = None;
                    self.page_image_clip = None;
                    self.last_copied_image = Some((width as u32, height as u32, rgba.clone()));
                }
                crate::create_ui::png_from_rgba(width, height, &rgba).ok().map(|b| ("pasted_image.png".to_string(), b))
            }
            Some(crate::create_ui::Clip::Text(_)) => None,
            None => {
                if matches!(system, Some(crate::create_ui::Clip::Text(_))) || (system_authoritative && system.is_none()) {
                    self.last_copied_image = None;
                    self.last_copied_file = None;
                    self.page_image_clip = None;
                    self.notify_tr("No image on clipboard to paste");
                    return false;
                }
                let sys_result = match system {
                    Some(crate::create_ui::Clip::Image { width, height, rgba }) => Some((width, height, rgba)),
                    _ => None,
                };

                if let Some((img_width, img_height, img_bytes)) = sys_result {
                    let is_same_as_last = self
                        .last_copied_image
                        .as_ref()
                        .is_some_and(|(w, h, rgba)| *w as usize == img_width && *h as usize == img_height && rgba.as_slice() == img_bytes.as_slice());
                    if is_same_as_last && let Some((name, bytes)) = self.last_copied_file.clone() {
                        Some((name, bytes))
                    } else if is_same_as_last && let Some((w, h, rgba)) = self.last_copied_image.clone() {
                        copied_here = true;
                        crate::create_ui::png_from_rgba(w as usize, h as usize, &rgba).ok().map(|b| ("pasted_image.png".to_string(), b))
                    } else {
                        // System clipboard has a newer / external image!
                        self.last_copied_file = None;
                        self.page_image_clip = None;
                        self.last_copied_image = Some((img_width as u32, img_height as u32, img_bytes.clone()));
                        crate::create_ui::png_from_rgba(img_width, img_height, &img_bytes).ok().map(|b| ("clipboard_image.png".to_string(), b))
                    }
                } else if let Some((name, bytes)) = self.last_copied_file.clone() {
                    Some((name, bytes))
                } else if let Some((w, h, rgba)) = self.last_copied_image.clone() {
                    copied_here = true;
                    crate::create_ui::png_from_rgba(w as usize, h as usize, &rgba).ok().map(|b| ("pasted_image.png".to_string(), b))
                } else {
                    None
                }
            }
        };

        let Some((name, bytes)) = image_data else {
            self.notify_tr("No image on clipboard to paste");
            return false;
        };
        if copied_here && rect.is_none() && self.paste_page_image_clip(id, page, at, &bytes, &name) {
            return true;
        }

        let target_rect = if let Some(r) = rect {
            Some(r)
        } else if let Some(at) = at {
            let (raw_w, raw_h) = self
                .last_copied_image
                .as_ref()
                .map(|(w, h, _)| (*w as f64, *h as f64))
                .or_else(|| {
                    (bytes.len() <= 64_000_000 * 4).then(|| pdfcraft_engine::image_dimensions(&bytes)).flatten().map(|(w, h)| (w as f64, h as f64))
                })
                .unwrap_or((100.0, 100.0));
            let (pw, ph) =
                self.session.get(id).and_then(|d| d.info.pages.get(page)).map(|p| (p.width as f64, p.height as f64)).unwrap_or((612.0, 792.0));
            let k = ((pw * 0.8) / raw_w.max(1.0)).min((ph * 0.8) / raw_h.max(1.0)).min(1.0);
            let (w, h) = (raw_w * k, raw_h * k);
            let Some(info) = self.session.get(id).and_then(|doc| doc.info.pages.get(page)) else { return false };
            let view = info.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
            // Added content uses the page's bottom-left display origin; rendered view space is y-down.
            let center = [view[0], f64::from(info.height) - view[1]];
            Some([center[0] - w * 0.5, center[1] - h * 0.5, center[0] + w * 0.5, center[1] + h * 0.5])
        } else {
            None
        };

        self.paste_image_edit(i, id, page, pdfcraft_engine::Edit::AddImage { page, rect: target_rect, name, bytes: std::sync::Arc::new(bytes) })
    }

    /// Apply `edit`, which pastes a picture onto `page` as an added one, then select what it
    /// added in Edit text & images.
    fn paste_image_edit(&mut self, i: usize, id: pdfcraft_engine::DocId, page: usize, edit: Edit) -> bool {
        let added_before: std::collections::HashSet<_> =
            self.session.get(id).into_iter().flat_map(|doc| doc.added()).filter(|item| item.page == page).map(|item| item.obj).collect();
        if !self.apply_edit(edit) {
            return false;
        }
        let selection = self.session.get(id).and_then(|doc| {
            let mut inserted = doc.added().iter().filter(|item| item.page == page).enumerate().filter(|(_, item)| !added_before.contains(&item.obj));
            let (added_idx, item) = inserted.next()?;
            if inserted.next().is_some() {
                return None;
            }
            let AddedContent::Image(image) = &item.content else { return None };
            let pasted_idx = doc.page_images(page).iter().rposition(|placed| placed.object == Some(image.image))?;
            Some((added_idx, pasted_idx))
        });
        if let Some(v) = self.views.get_mut(i) {
            v.current = page;
            v.content.selected = selection.map(|(added, _)| (page, added));
            v.content.select_added = v.content.selected;
            v.image_selection = selection.map(|(_, image)| crate::edit_text_ui::ImageSelection::at(page, image));
        }
        self.quick_tool = crate::QuickTool::EditText;
        self.notify_tr("Image pasted");
        true
    }
}

/// What the image tools ask for.
pub(crate) enum ImageAction {
    Update(pdfcraft_engine::AddedContent),
    Replace,
}

// Lucide names a flip icon after the axis it draws, Acrobat names the flip after the direction the
// picture moves: Flip horizontal (left and right swap) shows Lucide's vertical axis.
const FLIP_HORIZONTAL_ICON: &str = "flip-vertical-2";
const FLIP_VERTICAL_ICON: &str = "flip-horizontal-2";

/// Edit image: rotate, flip, crop, replace (shown while an added image is selected).
pub(crate) fn image_panel(ui: &mut egui::Ui, t: &Tokens, img: &pdfcraft_engine::AddedImage) -> Option<ImageAction> {
    let mut out = None;
    widgets::section_title(ui, tl!("Edit image"));
    ui.horizontal(|ui| {
        let mut i = img.clone();
        let r = i.rect;
        let turn = |i: &mut pdfcraft_engine::AddedImage, k: u8| {
            i.rotation = (i.rotation + k) % 4;
            // The box turns with the picture, around its centre.
            let (cx, cy, w, h) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, r[2] - r[0], r[3] - r[1]);
            i.rect = [cx - h / 2.0, cy - w / 2.0, cx + h / 2.0, cy + w / 2.0];
        };
        if crate::icons::button(ui, "rotate-ccw", 28.0, false, tl!("Rotate counterclockwise")).clicked() {
            turn(&mut i, 1);
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, "rotate-cw", 28.0, false, tl!("Rotate clockwise")).clicked() {
            turn(&mut i, 3);
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, FLIP_HORIZONTAL_ICON, 28.0, false, tl!("Flip horizontal")).clicked() {
            i.flip_h = !i.flip_h;
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, FLIP_VERTICAL_ICON, 28.0, false, tl!("Flip vertical")).clicked() {
            i.flip_v = !i.flip_v;
            out = Some(ImageAction::Update(AddedContent::Image(i.clone())));
        }
        if crate::icons::button(ui, "replace", 28.0, false, tl!("Replace image")).clicked() {
            out = Some(ImageAction::Replace);
        }
    });
    ui.label(egui::RichText::new(tl!("Crop (% trimmed from each side)")).small().color(t.text_muted));
    let most = (1.0 - pdfcraft_engine::AddedImage::MIN_CROP_KEEP) * 100.0;
    // Unrounded: a side the user doesn't touch keeps its stored value, also one read from a file.
    let shown = img.crop.map(|v| v * 100.0);
    let mut crop = shown;
    let mut changed = false;
    ui.horizontal(|ui| {
        for (k, label) in ["L", "B", "R", "T"].into_iter().enumerate() {
            ui.label(label);
            // Up to what the opposite side leaves. Applied when the drag ends: one undo step.
            let room = (most - crop.get((k + 2) % 4).copied().unwrap_or(0.0)).max(0.0);
            let Some(side) = crop.get_mut(k) else { continue };
            let r = ui.add(egui::DragValue::new(side).range(0.0..=room).clamp_existing_to_range(false).speed(0.5).suffix("%"));
            changed |= r.drag_stopped() || (r.changed() && !r.dragged());
        }
    });
    if changed {
        let mut i = img.clone();
        for (stored, (now, was)) in i.crop.iter_mut().zip(crop.iter().zip(&shown)) {
            if now != was {
                *stored = now / 100.0;
            }
        }
        if i.crop != img.crop {
            out = Some(ImageAction::Update(AddedContent::Image(i)));
        }
    }
    out
}

/// The editor's layout of added text with character spacing. The page puts the spacing after every
/// character, the last of a line included (`latin_width`), while epaint puts it only between
/// characters: the wrap width gives up one spacing so both break lines at the same place.
fn spaced_job(
    text: &str,
    font: &egui::FontId,
    color: Color32,
    letter_spacing: f32,
    underline: bool,
    wrap_width: f32,
    halign: egui::Align,
) -> egui::text::LayoutJob {
    let underline = if underline { Stroke::new(1.0, color) } else { Stroke::NONE };
    let format = egui::TextFormat { font_id: font.clone(), color, extra_letter_spacing: letter_spacing, underline, ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.wrap.max_width = (wrap_width - letter_spacing).max(0.0);
    job.halign = halign;
    // As the text field's own layout does: the page counts trailing spaces when it aligns a line,
    // so a centred or right-aligned line must too, or the editor shifts it.
    job.keep_trailing_whitespace = true;
    job
}

#[cfg(test)]
mod wheel_tests {
    use egui_kittest::kittest::Queryable;

    /// The wheel over the size box previews each step and applies once, when it rests.
    #[test]
    fn the_wheel_previews_the_size_then_applies_it_once() {
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, state: &mut (f64, Option<f64>, usize)| {
                let before = state.0;
                state.1 = None;
                ui.horizontal(|ui| super::size_control(ui, &mut state.0, &mut state.1, false));
                if state.0 != before {
                    state.2 += 1;
                }
            },
            (12.0, None, 0),
        );
        h.run();
        let box_at = h.get_by_role(egui::accesskit::Role::TextInput).rect().center();
        h.hover_at(box_at);
        h.run_steps(1);
        let notch = |up: f32| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, up),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        };
        h.event(notch(1.0));
        h.event(notch(1.0));
        h.input_mut().time = Some(100.0);
        h.step();
        h.input_mut().time = Some(100.1);
        h.step();
        assert_eq!(*h.state(), (12.0, Some(14.0), 0), "previewed, not applied");
        h.input_mut().time = Some(101.0);
        h.step();
        assert_eq!(*h.state(), (14.0, None, 1), "applied once, as the wheel rests");
        h.event(notch(-1.0));
        h.input_mut().time = Some(102.0);
        h.step();
        assert_eq!(h.state().1, Some(13.0), "down steps down");
        // With the caret in the box, the wheel works the same.
        h.input_mut().time = Some(104.0);
        h.step();
        assert_eq!(h.state().0, 13.0);
        h.get_by_role(egui::accesskit::Role::TextInput).click();
        h.run_steps(2);
        h.hover_at(box_at);
        h.run_steps(1);
        h.event(notch(1.0));
        h.input_mut().time = Some(200.0);
        h.step();
        h.input_mut().time = Some(201.0);
        h.step();
        assert_eq!(h.state().0, 14.0, "the wheel works while the box has the caret");
    }
}

#[cfg(test)]
mod snap_tests {
    use egui::{Rect, pos2, vec2};

    use super::{SNAP, backdrop, snap_delta};

    /// Thai edition: a scan covering the page is no box to line up with; a picture on part of
    /// it is.
    #[test]
    fn a_page_sized_picture_is_a_backdrop_and_not_a_snap_target() {
        let page = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 800.0));
        assert!(backdrop(page, page.expand(2.0).translate(vec2(1.5, 0.0))));
        assert!(backdrop(page, page.shrink(20.0)));
        assert!(!backdrop(page, Rect::from_min_size(pos2(120.0, 70.0), vec2(300.0, 200.0))));
        assert!(!backdrop(page, Rect::from_min_size(pos2(-900.0, 70.0), vec2(300.0, 200.0))));
    }

    /// A box moved to within a few points of the page's centre lines lands on them; farther away
    /// it moves exactly as dragged.
    #[test]
    fn a_moved_box_snaps_its_centre_onto_the_page_centre_lines() {
        let page = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 800.0));
        let r = Rect::from_min_size(pos2(120.0, 70.0), vec2(100.0, 40.0));
        // Its centre (170, 90) is dragged to (397, 448): 3 points short of (400, 450) both ways.
        let d = snap_delta(page, r, vec2(227.0, 358.0), &[], None);
        assert_eq!(r.center() + d, page.center());
        // Only the axis that is close snaps.
        let d = snap_delta(page, r, vec2(229.0, 100.0), &[], None);
        assert_eq!(((r.center() + d).x, (r.center() + d).y), (400.0, 190.0));
        // Beyond the snapping distance nothing changes.
        let far = vec2(230.0 - SNAP - 1.0, 100.0);
        assert_eq!(snap_delta(page, r, far, &[], None), far);
    }

    /// Thai edition: a moved box's edges and centre line up with another box's, as in Photoshop.
    #[test]
    fn a_moved_box_lines_up_with_the_edges_and_centres_of_other_boxes() {
        let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        let other = Rect::from_min_size(pos2(300.0, 100.0), vec2(200.0, 80.0));
        let r = Rect::from_min_size(pos2(40.0, 600.0), vec2(100.0, 50.0));
        // Left edge 4 points right of the other's left edge: it lands on it.
        let d = snap_delta(page, r, vec2(264.0, 0.0), &[other], None);
        assert_eq!(r.translate(d).left(), other.left());
        // Right edge 3 short of the other's right edge.
        let d = snap_delta(page, r, vec2(357.0, 0.0), &[other], None);
        assert_eq!(r.translate(d).right(), other.right());
        // Centre onto the other's centre, both ways.
        let d = snap_delta(page, r, other.center() - r.center() + vec2(2.0, -5.0), &[other], None);
        assert_eq!(r.translate(d).center(), other.center());
        // The top edge onto the other's bottom edge (stacking below it).
        let d = snap_delta(page, r, vec2(500.0, other.bottom() - r.top() + 4.0), &[other], None);
        assert_eq!(r.translate(d).top(), other.bottom());
        // Far from everything it moves as dragged.
        let far = vec2(700.0, -200.0);
        assert_eq!(snap_delta(page, r, far, &[other], None), far);
    }

    /// Thai edition: with the grid showing, a moved box's edge lands on the nearest grid line.
    #[test]
    fn a_moved_box_snaps_onto_the_grid() {
        let page = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 800.0));
        let r = Rect::from_min_size(pos2(113.0, 77.0), vec2(31.0, 30.0));
        // Moved by (35, 8): its left edge at 148, 2 short of the grid line at 150 (100 + 2 × 25).
        // Its top (85) and bottom (115) are 10 from the nearest lines (75, 125): they stay.
        let d = snap_delta(page, r, vec2(35.0, 8.0), &[], Some(25.0));
        assert_eq!(r.translate(d).left(), 150.0);
        assert_eq!(r.translate(d).top(), 85.0);
        // Without the grid it moves as dragged.
        assert_eq!(snap_delta(page, r, vec2(35.0, 8.0), &[], None), vec2(35.0, 8.0));
    }

    /// Thai edition (the owner's choice): aligning moves the text inside its box, which keeps its
    /// size; a box made wider than its text keeps its width when restyled; one that hugs its text
    /// still follows it.
    #[test]
    fn aligning_keeps_the_box_and_only_hugging_boxes_follow_their_text() {
        use super::{AddedText, TextAlign, restyled};
        let hug = pdfcraft_engine::fitted_text(
            &AddedText { text: "Hello".into(), rect: [100.0, 100.0, 400.0, 114.0], size: 12.0, ..Default::default() },
            600.0,
        );
        assert!(pdfcraft_engine::hugs_text(&hug, 600.0));
        let wide = AddedText { rect: [100.0, 100.0, 400.0, 114.0], ..hug.clone() };
        assert!(!pdfcraft_engine::hugs_text(&wide, 600.0));
        for old in [&hug, &wide] {
            let centred = restyled(old, AddedText { align: TextAlign::Center, ..old.clone() }, Some(600.0));
            assert_eq!(centred.rect, old.rect, "aligning keeps the box");
            assert_eq!(centred.align, TextAlign::Center);
        }
        let centre = |t: &AddedText| {
            let r = pdfcraft_engine::added_text_rect(t);
            [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
        };
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01;
        let bigger = restyled(&wide, AddedText { size: 18.0, ..wide.clone() }, Some(600.0));
        assert_eq!(bigger.rect[2] - bigger.rect[0], wide.rect[2] - wide.rect[0], "a wide box keeps its width");
        assert!(near(centre(&bigger), centre(&wide)), "and its centre: {:?} {:?}", centre(&bigger), centre(&wide));
        // Thai edition: a hugging box grows with its text about its centre, and shrinks back there.
        let bigger = restyled(&hug, AddedText { size: 48.0, ..hug.clone() }, Some(600.0));
        assert!(bigger.rect[2] - bigger.rect[0] > hug.rect[2] - hug.rect[0], "a hugging box grows with its text");
        assert!(near(centre(&bigger), centre(&hug)), "the text stays put: {:?} {:?}", centre(&bigger), centre(&hug));
        let smaller = restyled(&bigger, AddedText { size: 8.0, ..bigger.clone() }, Some(600.0));
        assert!(near(centre(&smaller), centre(&hug)), "shrinking too: {:?} {:?}", centre(&smaller), centre(&hug));
    }

    /// No ruler guides.
    const NO_LINES: (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());

    /// Thai edition: a moved box snaps onto a ruler guide dragged out of the rulers, and a resized
    /// edge does too.
    #[test]
    fn boxes_snap_to_ruler_guides() {
        use super::{Hold, snap_delta_held, snap_resized_held};
        let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        let r = Rect::from_min_size(pos2(100.0, 400.0), vec2(50.0, 20.0));
        let lines = (vec![255.0], vec![700.0]);
        // Its left edge 4 short of the upright guide at 255, its bottom 3 short of the level one.
        let d = snap_delta_held(page, r, vec2(151.0, 277.0), &[], None, &lines, &mut Hold::default());
        assert_eq!(r.translate(d).left(), 255.0);
        assert_eq!(r.translate(d).bottom(), 700.0);
        // Without the guides it moves as dragged.
        assert_eq!(snap_delta_held(page, r, vec2(151.0, 277.0), &[], None, &NO_LINES, &mut Hold::default()), vec2(151.0, 277.0));
        let to = Rect::from_min_max(r.min, pos2(252.0, 420.0));
        let out = snap_resized_held(page, to, (Some(false), None), false, &[], (None, &lines), &mut Hold::default());
        assert_eq!(out.right(), 255.0);
    }

    /// Thai edition: a box that snapped to a guide is held on it while the drag pulls a little
    /// past the catch distance, as in Photoshop, and lets go further out.
    #[test]
    fn a_snapped_box_is_held_until_pulled_clearly_away() {
        use super::{Hold, snap_delta_held};
        let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        let other = Rect::from_min_size(pos2(300.0, 100.0), vec2(300.0, 40.0));
        let r = Rect::from_min_size(pos2(100.0, 400.0), vec2(50.0, 20.0));
        let mut hold = Hold::default();
        // Its left edge 4 from the other's left (300): it snaps, and holds the guide.
        let d = snap_delta_held(page, r, vec2(196.0, 0.0), &[other], None, &NO_LINES, &mut hold);
        assert_eq!(r.translate(d).left(), 300.0);
        assert!(hold.x.is_some());
        // 10 past it, beyond the catch distance but within the hold: still on the guide.
        let d = snap_delta_held(page, r, vec2(210.0, 0.0), &[other], None, &NO_LINES, &mut hold);
        assert_eq!(r.translate(d).left(), 300.0, "held");
        // Without the hold the same move snaps to nothing.
        assert_eq!(snap_delta(page, r, vec2(210.0, 0.0), &[other], None).x, 210.0);
        // 20 past it: let go, and the hold is gone.
        let d = snap_delta_held(page, r, vec2(220.0, 0.0), &[other], None, &NO_LINES, &mut hold);
        assert_eq!(d.x, 220.0);
        assert!(hold.x.is_none());
    }

    /// Thai edition: a box resized by a corner or a side snaps only the edges being dragged onto
    /// the guides; the edges that stay put stay put.
    #[test]
    fn a_resized_box_snaps_the_dragged_edges() {
        use super::{Hold, snap_resized_held};
        let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        let other = Rect::from_min_max(pos2(300.0, 100.0), pos2(500.0, 180.0));
        let snap = |to: Rect, edges, keep| snap_resized_held(page, to, edges, keep, &[other], (None, &NO_LINES), &mut Hold::default());
        // The bottom-right corner dragged to 4 short of the other's right edge, far from any
        // horizontal guide: the right edge lands on 500, the rest as dragged.
        let to = Rect::from_min_max(pos2(100.0, 400.0), pos2(496.0, 437.0));
        assert_eq!(snap(to, (Some(false), Some(false)), false), Rect::from_min_max(pos2(100.0, 400.0), pos2(500.0, 437.0)));
        // A left side dragged 3 past the other's left edge: only the left edge moves onto it.
        let to = Rect::from_min_max(pos2(297.0, 400.0), pos2(700.0, 437.0));
        assert_eq!(snap(to, (Some(true), None), false), Rect::from_min_max(pos2(300.0, 400.0), pos2(700.0, 437.0)));
        // A right side doesn't snap its left edge, even when that one is near a guide.
        assert_eq!(snap(to, (Some(false), None), false), to);
        // Beyond the snapping distance it resizes as dragged.
        let to = Rect::from_min_max(pos2(100.0, 400.0), pos2(500.0 - SNAP - 1.0, 437.0));
        assert_eq!(snap(to, (Some(false), Some(false)), false), to);
    }

    /// Thai edition: a picture resized by a corner keeps its proportions: the edge nearer a guide
    /// snaps and the other follows, around the corner that stays put.
    #[test]
    fn a_picture_resized_by_a_corner_keeps_its_proportions_when_it_snaps() {
        use super::{Hold, snap_resized_held};
        let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        let other = Rect::from_min_max(pos2(300.0, 100.0), pos2(500.0, 180.0));
        let mut hold = Hold::default();
        // 2:1, its top-left fixed at (100, 400), the corner dragged to 4 short of x = 500.
        let to = Rect::from_min_max(pos2(100.0, 400.0), pos2(496.0, 598.0));
        let out = snap_resized_held(page, to, (Some(false), Some(false)), true, &[other], (None, &NO_LINES), &mut hold);
        assert_eq!(out.min, to.min, "the opposite corner stays");
        assert_eq!(out.right(), 500.0);
        assert!((out.width() / out.height() - 2.0).abs() < 1e-4, "still 2:1: {out:?}");
        // Held while pulled a little past the guide.
        let to = Rect::from_min_max(pos2(100.0, 400.0), pos2(510.0, 605.0));
        let out = snap_resized_held(page, to, (Some(false), Some(false)), true, &[other], (None, &NO_LINES), &mut hold);
        assert_eq!(out.right(), 500.0, "held");
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Thai edition: the font size box holds the largest size, three digits, inside its margins,
    /// so "72" or "500" shows whole.
    #[test]
    fn the_size_box_is_wide_enough_for_three_digits() {
        let mut fonts = false;
        let mut h = egui_kittest::Harness::new_ui_state(
            move |ui, size: &mut f64| {
                if !fonts {
                    ui.ctx().set_fonts(crate::theme::font_definitions());
                    fonts = true;
                    return;
                }
                ui.horizontal(|ui| size_control(ui, size, &mut None, false));
            },
            500.0,
        );
        h.run();
        let rect = h.get_by_label("Font size").rect();
        let font = egui::TextStyle::Body.resolve(&h.ctx.global_style());
        let digits = h.ctx.fonts_mut(|f| f.layout_no_wrap("500".to_owned(), font, egui::Color32::BLACK).size().x);
        // The box's own margins on both sides, and the text.
        assert!(f64::from(rect.width()) >= f64::from(digits) + 8.0, "{} wide for {digits} of digits", rect.width());
    }

    #[test]
    fn delivered_pixels_are_bounded_before_cache_or_document_changes() {
        let mut app = crate::PdfCraftApp::new();
        app.create_blank();
        app.last_copied_image = Some((1, 1, vec![0, 255, 0, 255]));
        let before = app.last_copied_image.clone();
        for size in [[0, 1], [usize::MAX, 2], [crate::create_ui::MAX_IMAGE_SIDE + 1, 1], [2, 2]] {
            let mut image = egui::ColorImage::filled([1, 1], egui::Color32::RED);
            image.size = size;
            assert!(!app.paste_received_image(0, &image));
            assert_eq!(app.last_copied_image, before);
        }
        assert!(app.session.get(app.views[0].id).unwrap().page_images(0).is_empty());
    }

    #[test]
    fn delivered_pixels_keep_unpremultiplied_alpha() {
        let mut app = crate::PdfCraftApp::new();
        app.create_blank();
        let image = egui::ColorImage::from_rgba_unmultiplied([1, 1], &[255, 0, 0, 128]);
        assert!(app.paste_received_image(0, &image));
        assert_eq!(app.last_copied_image.unwrap().2, vec![255, 0, 0, 128]);
    }

    #[test]
    fn review_web_can_paste_an_internal_image_without_a_native_clipboard() {
        let mut app = crate::PdfCraftApp::new();
        app.create_blank();
        app.system_clipboard = true;
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        assert!(app.paste_image_with_system(0, None, None, None, false, None));
        let id = app.active_ids().unwrap().1;
        assert_eq!(app.session.get(id).unwrap().page_images(0).len(), 1);
    }

    #[test]
    fn review_system_without_supported_formats_does_not_paste_a_cached_image() {
        let mut app = crate::PdfCraftApp::new();
        app.create_blank();
        app.system_clipboard = true;
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        assert!(!app.paste_image_with_system(0, None, None, None, true, None));
    }

    #[test]
    fn review_cut_checks_modify_permission_before_writing_the_clipboard() {
        let mut app = crate::PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        let png = crate::create_ui::png_from_rgba(1, 1, &[255, 0, 0, 255]).unwrap();
        app.add_image("test.png".into(), png);
        app.apply_edit(Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            changes: pdfcraft_engine::Changes::None,
            copy: true,
            ..Default::default()
        }));
        let id = app.active_ids().unwrap().1;
        let bytes = app.session.save_bytes(id).unwrap();
        let mut locked = crate::PdfCraftApp::new();
        locked.system_clipboard = false;
        locked.open_bytes("locked.pdf", None, bytes.as_ref().clone()).unwrap();
        assert!(!locked.session.get(locked.active_ids().unwrap().1).unwrap().allows_modification());
        locked.last_copied_image = Some((1, 1, vec![0, 255, 0, 255]));
        let before = locked.last_copied_image.clone();
        assert!(!locked.cut_page_image(0, 0));
        assert_eq!(locked.last_copied_image, before);
    }

    #[test]
    fn system_text_prevents_context_menu_paste_of_a_cached_image() {
        let mut app = crate::PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        let id = app.active_ids().unwrap().1;
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        let undo = app.session.get(id).unwrap().can_undo().map(str::to_owned);
        assert!(!app.paste_image_with_system(0, None, Some([20.0, 20.0]), Some(crate::create_ui::Clip::Text("new text".into())), true, None));
        assert_eq!(app.session.get(id).unwrap().can_undo().map(str::to_owned), undo);
        assert!(app.session.get(id).unwrap().page_images(0).is_empty());
        assert!(app.last_copied_image.is_none());
    }

    /// The image a flip button leaves behind, as the panel returns it.
    fn flipped(tooltip: &str) -> pdfcraft_engine::AddedImage {
        let img = pdfcraft_engine::AddedImage::new([0.0, 0.0, 100.0, 50.0], pdfcraft_cos::ObjRef::new(7, 0));
        let mut fonts = false;
        let mut h = egui_kittest::Harness::new_ui_state(
            move |ui, out: &mut Option<pdfcraft_engine::AddedImage>| {
                // The app's fonts (the semibold section title) take effect from the next frame.
                if !fonts {
                    ui.ctx().set_fonts(crate::theme::font_definitions());
                    fonts = true;
                    return;
                }
                let t = Tokens::get(ui.ctx());
                if let Some(ImageAction::Update(AddedContent::Image(i))) = image_panel(ui, &t, &img) {
                    *out = Some(i);
                }
            },
            None,
        );
        h.run();
        h.get_by_label(tooltip).click();
        h.run();
        h.state().clone().expect("the button updates the image")
    }

    #[test]
    fn flip_buttons_show_the_mirror_axis_of_the_flip_they_make() {
        // #1834: Acrobat's Flip horizontal swaps left and right across a vertical axis. Lucide names
        // its flip icons after the axis drawn, so that is its flip-vertical-2.
        let svg = |name: &str| {
            let (_, bytes) = crate::icon_data::ICONS.iter().find(|(n, _)| *n == name).expect("bundled icon");
            String::from_utf8_lossy(bytes).into_owned()
        };
        let (h, v) = (svg(FLIP_HORIZONTAL_ICON), svg(FLIP_VERTICAL_ICON));
        assert!(h.contains("M12 2v2") && !h.contains("M4 12H2"), "Flip horizontal draws a vertical axis:\n{h}");
        assert!(v.contains("M4 12H2") && !v.contains("M12 2v2"), "Flip vertical draws a horizontal axis:\n{v}");

        let i = flipped("Flip horizontal");
        assert!(i.flip_h && !i.flip_v, "Flip horizontal mirrors left and right");
        let i = flipped("Flip vertical");
        assert!(i.flip_v && !i.flip_h, "Flip vertical mirrors top and bottom");
    }

    /// #4464: both rasters were in at one scale and a render at a new scale failed. The rasters
    /// from the old scale must go too, or the item keeps following the pointer although the
    /// preview gave up.
    #[test]
    fn failed_rerender_drops_the_rasters_from_the_earlier_scale() {
        let ctx = egui::Context::default();
        let texture = |name| Some(ctx.load_texture(name, egui::ColorImage::filled([1, 1], Color32::WHITE), egui::TextureOptions::LINEAR));
        // Bytes no parser accepts: every render answers with an error.
        let pool = || {
            let pool = pdfcraft_render::RenderPool::new_inline(std::sync::Arc::new(b"not a pdf".to_vec()), Default::default());
            pool.set_queue(vec![pdfcraft_render::RenderRequest { page: 0, scale: 2.0, tag: 2000, ..Default::default() }]);
            pool
        };
        let mut mb = MoveBackground {
            key: Some((0, 0, 0)),
            renderer: Some(pool()),
            texture: texture("background"),
            item_renderer: Some(pool()),
            item: texture("item"),
            tag: Some(2000),
            from: Some([0.0, 0.0, 10.0, 10.0]),
            ..Default::default()
        };
        assert!(mb.ready(), "both rasters from the earlier scale are in");
        assert!(!mb.receive(&ctx, 2000), "the new scale's renders fail");
        assert!(mb.texture.is_none() && mb.item.is_none(), "no raster is left to draw the moving item from");
        assert!(!mb.ready() && !mb.pending());
    }

    /// #4245: the page counts the spacing after a line's last character too, so a line that fits
    /// only without it wraps in the saved text and must wrap in the editor.
    #[test]
    fn editor_wraps_where_the_saved_text_does_with_trailing_spacing() {
        let rows = |wrap: f32| {
            let mut n = 0;
            let mut h = egui_kittest::Harness::new_ui_state(
                move |ui, n: &mut usize| {
                    let font = egui::FontId::proportional(14.0);
                    let job = spaced_job("AAAA BBBB", &font, Color32::BLACK, 10.0, false, wrap, egui::Align::Min);
                    *n = ui.fonts_mut(|f| f.layout_job(job)).rows.len();
                },
                n,
            );
            h.run();
            n = *h.state();
            n
        };
        // The text's width without the trailing spacing, as epaint measures it.
        let one_line = rows(f32::INFINITY);
        assert_eq!(one_line, 1);
        let width = {
            let mut w = 0.0;
            let mut h = egui_kittest::Harness::new_ui_state(
                |ui, w: &mut f32| {
                    let font = egui::FontId::proportional(14.0);
                    let job = spaced_job("AAAA BBBB", &font, Color32::BLACK, 10.0, false, f32::INFINITY, egui::Align::Min);
                    *w = ui.fonts_mut(|f| f.layout_job(job)).rect.width();
                },
                w,
            );
            h.run();
            w = *h.state();
            w
        };
        // Page width = width + 10 (spacing after the last character) > width + 5: the page wraps.
        assert_eq!(rows(width + 5.0), 2, "the saved text wraps here, so the editor must too");
        assert_eq!(rows(width + 10.0), 1, "and keeps the line when the spacing fits");
    }
}
