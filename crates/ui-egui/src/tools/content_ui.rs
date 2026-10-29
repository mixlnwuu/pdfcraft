//! Edit a PDF ▸ Add content: the Text tool (click on the page and type), Image (pick a file;
//! it lands in the middle of the page), and editing what was added: select, move, resize
//! (images keep their proportions), double-click text to retype it, format it from the panel,
//! Delete to remove it. Each change is one undoable engine edit.

use crate::widgets::Tip as _;
use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{Added, AddedContent, AddedText, Arrange, Edit, FontFamily, TextAlign};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_pointer::PagePointer;
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
    pub style: AddedText,
    pub focus: bool,
    /// The box hugs the text: new text, and retyped text whose box didn't wrap it. A box made
    /// narrow enough to wrap keeps its width.
    pub auto: bool,
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
            let d = if grab == Grab::Move { snap_to_centre(ui, xf.rect, r, p - start) } else { p - start };
            dragged(r, grab, d, self.picture)
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
    on_page(added, page).iter().any(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(point))
}

fn handles(r: Rect) -> [(Pos2, bool, bool); 4] {
    [(r.left_top(), true, true), (r.right_top(), false, true), (r.left_bottom(), true, false), (r.right_bottom(), false, false)]
}

/// How close (screen points) a moved box's centre comes to the page's centre line before it
/// snaps onto it.
const SNAP: f32 = 6.0;

/// Smart guides (Thai edition), as in Word: the move `d` of a box `r` (screen), adjusted so the
/// box's centre lands on the page's centre lines when it comes within [`SNAP`] of them. Alt held
/// moves freely.
pub(crate) fn snap_to_centre(ui: &egui::Ui, page: Rect, r: Rect, d: egui::Vec2) -> egui::Vec2 {
    if ui.input(|i| i.modifiers.alt) { d } else { snap_delta(page, r, d) }
}

/// [`snap_to_centre`] without the Alt key.
fn snap_delta(page: Rect, r: Rect, d: egui::Vec2) -> egui::Vec2 {
    let (c, pc) = (r.center() + d, page.center());
    let mut out = d;
    if (c.x - pc.x).abs() <= SNAP {
        out.x += pc.x - c.x;
    }
    if (c.y - pc.y).abs() <= SNAP {
        out.y += pc.y - c.y;
    }
    out
}

/// The page's centre lines a box being moved sits on, drawn across the page in green.
pub(crate) fn paint_guides(painter: &egui::Painter, page: Rect, r: Rect) {
    let guide = Stroke::new(1.0, Color32::from_rgb(0x2E, 0xB8, 0x5C));
    let pc = page.center();
    if (r.center().x - pc.x).abs() < 0.5 {
        painter.line_segment([Pos2::new(pc.x, page.top()), Pos2::new(pc.x, page.bottom())], guide);
    }
    if (r.center().y - pc.y).abs() < 0.5 {
        painter.line_segment([Pos2::new(page.left(), pc.y), Pos2::new(page.right(), pc.y)], guide);
    }
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

/// Added text and images keep pointer ownership over a form field while editing content.
pub(crate) fn select_grabs(xf: &PageXform, ptr: &PagePointer, page: usize, info: &DocInfo, added: &[Added], view: &DocView) -> bool {
    let cv = &view.content;
    if cv.grab.is_some_and(|(grab_page, _, _)| grab_page == page) {
        return true;
    }
    let Some(p) = ptr.within(xf.rect) else { return false };
    on_page(added, page).into_iter().any(|(index, item)| {
        let rect = screen_rect(xf, info, page, item.content.rect());
        rect.expand(2.0).contains(p)
            || cv.selected == Some((page, index)) && handles(rect).into_iter().any(|(corner, _, _)| corner.distance(p) <= 6.0)
    })
}

/// Thai edition: in Edit text & images a corner of the selected added item (and a move or
/// resize under way) takes the pointer ahead of the page's own paragraphs and images, whose
/// boxes often lie under a new picture's corners.
pub(crate) fn handle_grabs(xf: &PageXform, ptr: &PagePointer, page: usize, info: &DocInfo, added: &[Added], view: &DocView) -> bool {
    let cv = &view.content;
    if cv.grab.is_some_and(|(grab_page, _, _)| grab_page == page) {
        return true;
    }
    let Some(p) = ptr.within(xf.rect) else { return false };
    on_page(added, page)
        .into_iter()
        .filter(|(index, _)| cv.selected == Some((page, *index)))
        .any(|(_, item)| handles(screen_rect(xf, info, page, item.content.rect())).into_iter().any(|(corner, _, _)| corner.distance(p) <= 6.0))
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
                && let Some((_, a)) = on_page(added, page).into_iter().find(|(i, _)| *i == si)
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
                let r = dragged(from, grab, d, keep);
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
        let hit = on_page(added, page).into_iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(p));
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
        let items = on_page(added, page);
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
    let items = on_page(added, page);
    let hit = items.iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(p)).map(|(i, a)| (*i, *a));
    let selected_rect = cv
        .selected
        .filter(|s| s.0 == page)
        .and_then(|(_, si)| items.iter().find(|(i, _)| *i == si))
        .map(|(_, a)| screen_rect(xf, info, page, a.content.rect()));
    let corner = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(p) <= 6.0));
    // Cursors only where the page shows: the mode bar and side panels over a page scrolled under
    // them keep their own (#2755).
    let shows = resp.contains_pointer();
    if adding_text && hit.is_none() {
        if shows {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if resp.clicked() {
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
            cv.draft = Some(TextDraft { page, index: None, rect, text: String::new(), style, focus: true, auto: true });
            consumed = true;
            if typed.is_some() {
                view.pending_edit = typed;
            }
        }
        return consumed;
    }
    if let Some((_, left, top)) = corner.filter(|_| shows) {
        ui.ctx().set_cursor_icon(if left == top { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
    } else if hit.is_some() && shows {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }
    if resp.drag_started() {
        let origin = ptr.origin.unwrap_or(p);
        if let Some((_, left, top)) = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(origin) <= 6.0)) {
            cv.grab = Some((page, Grab::Corner(left, top), origin));
            consumed = true;
        } else if let Some((i, _)) = items.iter().rev().find(|(_, a)| screen_rect(xf, info, page, a.content.rect()).expand(2.0).contains(origin)) {
            cv.selected = Some((page, *i));
            cv.grab = Some((page, Grab::Move, origin));
            consumed = true;
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
        let auto = !pdfcraft_engine::text_is_wrapped(t);
        cv.draft = Some(TextDraft { page, index: Some(i), rect: t.rect, text: t.text.clone(), style: t.clone(), focus: true, auto });
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
        if cv.draft.as_ref().is_some_and(|d| d.page == page && d.index == Some(i)) {
            continue;
        }
        let mut r = screen_rect(xf, info, page, a.content.rect());
        let selected = cv.selected == Some((page, i));
        if selected
            && let (Some((gp, g, start)), Some(p)) = (cv.grab, pointer)
            && gp == page
        {
            let d = if g == Grab::Move { snap_to_centre(ui, xf.rect, r, p - start) } else { p - start };
            r = dragged(r, g, d, matches!(a.content, AddedContent::Image(_)));
            if g == Grab::Move {
                paint_guides(painter, xf.rect, r);
            }
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

/// Delete removes the selected item; Escape clears the selection; the arrow keys move it.
pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView, added: &[Added], (layer, covered): (Option<(usize, usize)>, usize)) {
    if view.content.selected.is_none() || view.content.draft.is_some() || ctx.egui_wants_keyboard_input() {
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
    let Some(page) = view.content.draft.as_ref().map(|d| d.page) else { return (None, false) };
    let Some(xf) = view.page_xform(page) else { return (None, false) };
    let zoom = xf.zoom();
    let Some(draft) = view.content.draft.as_mut() else { return (None, false) };
    let font = crate::edit_text_ui::editor_font(ctx, &draft.style, (draft.style.size as f32 * zoom).clamp(1.0, 300.0));
    let text = AddedText { text: draft.text.clone(), ..draft.style.clone() };
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
    let width = if hugs { r.width() + 2.0 * EDITOR_MARGIN } else { r.width() }.max(60.0);
    let anchor = match draft.style.align {
        pdfcraft_engine::TextAlign::Center => 0.5,
        pdfcraft_engine::TextAlign::Right => 1.0,
        pdfcraft_engine::TextAlign::Left | pdfcraft_engine::TextAlign::Justify => 0.0,
    };
    let at = r.min - egui::vec2((width - r.width()) * anchor, 0.0);
    let background_ready = draft.index.is_some_and(|index| view.text_background.ready_added(page, index));
    let tokens = Tokens::get(ctx);
    let (mut commit, mut cancel, mut done) = (false, false, false);
    let mut undrawable = None;
    let memo = &view.content.undrawable;
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
            // Laid out as the page draws it, so opening the box doesn't show it realigned.
            // shortcut: justified text is shown left-aligned here (TextEdit can't justify), until the editor gets its own layouter.
            let halign = match t.style.align {
                pdfcraft_engine::TextAlign::Center => egui::Align::Center,
                pdfcraft_engine::TextAlign::Right => egui::Align::Max,
                pdfcraft_engine::TextAlign::Left | pdfcraft_engine::TextAlign::Justify => egui::Align::Min,
            };
            // Character spacing shows as the page will draw it, scaled like the font.
            let letter_spacing = (t.style.char_spacing * f64::from(zoom)) as f32;
            let mut spaced = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                let wrap_width = if hugs { f32::INFINITY } else { wrap_width };
                let job = spaced_job(text.as_str(), &font, Color32::from_rgb(cr, cg, cb), letter_spacing, wrap_width, halign);
                ui.fonts_mut(|f| f.layout_job(job))
            };
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut edit = egui::TextEdit::multiline(&mut t.text)
                    .font(font.clone())
                    .desired_width(width)
                    .desired_rows(1)
                    .align(egui::Align2::new(halign, egui::Align::TOP))
                    // Keep the page editor's original inset; the theme's larger button padding
                    // otherwise shifts narrow right-aligned boxes away from their text edge.
                    .margin(egui::Margin::symmetric(EDITOR_MARGIN as i8, 2))
                    .background_color(if background_ready { Color32::TRANSPARENT } else { Color32::WHITE })
                    .text_color(Color32::from_rgb(cr, cg, cb))
                    .hint_text(tl!("Type text"))
                    .id_salt("added-text-edit");
                if letter_spacing != 0.0 || hugs {
                    edit = edit.layouter(&mut spaced);
                }
                let resp = ui.add(edit).accessible_name(tl!("Added text"));
                if t.focus {
                    resp.request_focus();
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
                // Pressing a button takes the focus from the text: that is not a click away.
                let on_buttons = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| buttons.contains(p));
                cancel |= ui.input(|i| i.key_pressed(egui::Key::Escape));
                // Ctrl+Enter (⌘Enter) is Done, as it applies an edit in Edit text (Thai edition).
                done |= ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
                commit = done || resp.lost_focus() && !on_buttons && !view.objects.hold_editor;
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
    let text = d.text.trim_end().to_string();
    // A box that hugs its text is refitted to what was typed.
    let fit = |t: AddedText| match info.pages.get(d.page) {
        Some(p) if d.auto => pdfcraft_engine::fitted_text(&t, p.width as f64),
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

/// A restyled item: one whose box hugged its text is refitted to the new font or size.
pub(crate) fn restyled(old: &AddedText, new: AddedText, page_width: Option<f64>) -> AddedText {
    match page_width {
        Some(w) if !pdfcraft_engine::text_is_wrapped(old) => pdfcraft_engine::fitted_text(&new, w),
        _ => new,
    }
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

/// The text size: a box to type any size from 1 to 500 points into (Enter, or a click away,
/// applies it), with the usual sizes in a list beside it. Each applied size is one undo step.
/// `hovered` gets the listed size under the pointer.
pub(crate) fn size_control(ui: &mut egui::Ui, size: &mut f64, hovered: &mut Option<f64>) {
    let shown = |v: f64| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
    let id = ui.make_persistent_id("font-size-typed");
    // What is in the box, and the size it was typed over. The text is kept while the box has the
    // caret (also across the window losing focus) and follows the size when that changes
    // elsewhere: a preset, or another item selected.
    let kept: Option<(String, f64)> = ui.data(|d| d.get_temp(id));
    let mut typed = kept.filter(|(_, over)| *over == *size).map_or_else(|| shown(*size), |(text, _)| text);
    // Narrow, with no unit beside it, so the Format text row keeps its width (the tip says what it is):
    // as wide as the largest size, three digits, in the box's own font, with its margins (the width
    // given is the whole box's) and a point for the caret.
    const MARGIN: i8 = 4;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let digits = ui.fonts_mut(|f| f.layout_no_wrap("500".to_owned(), font, egui::Color32::BLACK).size().x);
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
    // An arrow alone: the tip names it, for the pointer and for assistive tech.
    presets.response.tip(tl!("Common font sizes"));
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
    /// Paragraphs only: added text has no underline yet.
    pub underline: Option<bool>,
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
                    .add_filter(tl!("Images"), &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"])
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
    pub(crate) fn copy_page_image(&mut self, page: usize, index: usize) -> bool {
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
                arboard::Clipboard::new().and_then(|mut c| c.set_image(img)).map_err(|e| e.to_string())?;
            }
            #[cfg(target_arch = "wasm32")]
            if self.system_clipboard {
                return Err("the web version can't put images on the clipboard yet; use Save Image As".to_string());
            }
            self.last_copied_image = Some((w, h, rgba));
            Ok((w, h))
        });
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

    /// The picture's visible box on its page, in user space.
    fn page_image_box(&self, page: usize, index: usize) -> Option<[f64; 4]> {
        let (_, id) = self.active_ids()?;
        let image = self.session.get(id)?.page_images(page).into_iter().nth(index)?;
        Some(image.visible_rect.unwrap_or(image.rect))
    }

    /// Cut: copy the picture, and only when that worked take it off the page (one undo step).
    pub(crate) fn cut_page_image(&mut self, page: usize, index: usize) {
        if self.copy_page_image(page, index) {
            self.apply_edit(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Delete });
        }
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
        let before = self.session.get(id).map_or(0, |d| d.added().iter().filter(|a| a.page == page).count());
        if self.apply_edit(Edit::AddImage { page, rect: None, name, bytes: std::sync::Arc::new(bytes) }) {
            self.views[i].content.select_added = Some((page, before));
            self.left = crate::LeftPanel::Tool("edit");
            self.left_open = true;
        }
    }

    /// Whether ⌘V / Ctrl+V was let go this frame. egui-winit swallows that press (with a Paste
    /// event only if the clipboard holds text), while a plain V press always comes through: a
    /// release of V with no press of V since the last one is the shortcut, even when ⌘ or Ctrl
    /// was let go first. Read every frame, dialogs or not, so a press is never missed.
    pub(crate) fn paste_key_released(&mut self, ctx: &egui::Context) -> bool {
        let mut released = false;
        ctx.input(|inp| {
            for e in &inp.events {
                if let egui::Event::Key { key: egui::Key::V, pressed, .. } = e {
                    released |= !*pressed && !self.v_pressed;
                    self.v_pressed = *pressed;
                }
            }
        });
        released
    }

    /// ⌘V / Ctrl+V with a picture on the system clipboard (a screenshot, a picture copied in a
    /// browser) places it on the current page, as Add Image does.
    /// Organize Pages keeps ⌘V for pages, Prepare Form for fields, and text fields for text.
    pub(crate) fn paste_image_key(&mut self, i: usize, ctx: &egui::Context, pasted: bool) {
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input();
        let Some(view) = self.views.get(i) else { return };
        if !pasted || !free || view.organize || view.forms.focus.is_some() || self.is_preparing() {
            return;
        }
        let clip = match self.clipboard_override.clone() {
            Some(c) => Some(c),
            #[cfg(not(target_arch = "wasm32"))]
            None if self.system_clipboard => crate::create_ui::read_clipboard(),
            None => None,
        };
        let Some(crate::create_ui::Clip::Image { width, height, rgba }) = clip else { return };
        match crate::create_ui::png_from_rgba(width, height, &rgba) {
            Ok(png) => self.add_image("Pasted image.png".into(), png),
            Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", tl!("Clipboard")), ("e", &e)]),
        }
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
fn spaced_job(text: &str, font: &egui::FontId, color: Color32, letter_spacing: f32, wrap_width: f32, halign: egui::Align) -> egui::text::LayoutJob {
    let format = egui::TextFormat { font_id: font.clone(), color, extra_letter_spacing: letter_spacing, ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.wrap.max_width = (wrap_width - letter_spacing).max(0.0);
    job.halign = halign;
    job
}

#[cfg(test)]
mod snap_tests {
    use egui::{Rect, pos2, vec2};

    use super::{SNAP, snap_delta};

    /// A box moved to within a few points of the page's centre lines lands on them; farther away
    /// it moves exactly as dragged.
    #[test]
    fn a_moved_box_snaps_its_centre_onto_the_page_centre_lines() {
        let page = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 800.0));
        let r = Rect::from_min_size(pos2(120.0, 70.0), vec2(100.0, 40.0));
        // Its centre (170, 90) is dragged to (397, 448): 3 points short of (400, 450) both ways.
        let d = snap_delta(page, r, vec2(227.0, 358.0));
        assert_eq!(r.center() + d, page.center());
        // Only the axis that is close snaps.
        let d = snap_delta(page, r, vec2(229.0, 100.0));
        assert_eq!(((r.center() + d).x, (r.center() + d).y), (400.0, 190.0));
        // Beyond the snapping distance nothing changes.
        let far = vec2(230.0 - SNAP - 1.0, 100.0);
        assert_eq!(snap_delta(page, r, far), far);
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
                ui.horizontal(|ui| size_control(ui, size, &mut None));
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
                    let job = spaced_job("AAAA BBBB", &font, Color32::BLACK, 10.0, wrap, egui::Align::Min);
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
                    let job = spaced_job("AAAA BBBB", &font, Color32::BLACK, 10.0, f32::INFINITY, egui::Align::Min);
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
