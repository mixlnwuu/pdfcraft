//! Thai edition: a page's own picture dragged in Edit text and images: the picture alone follows
//! the pointer, and dropped on another page it moves there.
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// A half-transparent picture filling a third of page 1, and a blank page 2 the same size.
fn two_pages() -> Harness<'static, PdfCraftApp> {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(80, 40, image::Rgba([200, 40, 40, 128]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("picture.png", None, png.clone()).expect("opens");
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    let full = h.state().session.get(id).unwrap().page_images(0)[0].rect;
    let (w, ht) = (full[2] - full[0], full[3] - full[1]);
    let (mx, my) = ((full[0] + full[2]) / 2.0, (full[1] + full[3]) / 2.0);
    h.state_mut().apply_edit(pdfcraft_engine::Edit::EditPageImage {
        page: 0,
        index: 0,
        change: pdfcraft_engine::ImageEdit::Move([mx - w / 6.0, my - ht / 6.0, mx + w / 6.0, my + ht / 6.0]),
    });
    h.state_mut().apply_edit(pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: w, height: ht });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().views[0].set_zoom(6.0);
    h.run_steps(6);
    h
}

/// Press, move in steps and (unless `hold`) release.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: egui::Pos2, to: egui::Pos2, hold: bool) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=6 {
        h.hover_at(from + (to - from) * (k as f32 / 6.0));
        h.run_steps(1);
    }
    if !hold {
        h.drop_at(to);
        h.run_steps(4);
    }
}

#[test]
fn a_page_picture_dragged_shows_alone_and_dropped_on_another_page_moves_there() {
    let mut h = two_pages();
    let id = h.state().views[0].id;
    let (p1, p2) =
        (h.state().views[0].page_screen_rect(0).expect("page 1 on screen"), h.state().views[0].page_screen_rect(1).expect("page 2 on screen"));
    let c = p1.center();
    let before = h.state().session.get(id).unwrap().page_images(0)[0].rect;
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some(), "selected");

    // Held over page 2: what follows the pointer is the picture's own pixels, drawn there, not a
    // piece of page 1's raster (which has the page behind the picture too).
    drag(&mut h, c, p2.center(), true);
    let tex = h.state().views[0].image_drag_picture.as_ref().and_then(|(_, t)| t.as_ref()).map(|t| t.id()).expect("the picture's pixels are in");
    let drawn: Vec<egui::Rect> = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Mesh(m) if m.texture_id == tex => Some(m.calc_bounds()),
            _ => None,
        })
        .collect();
    assert!(drawn.iter().any(|b| b.contains(p2.center())), "the picture alone follows the pointer onto page 2: {drawn:?}");
    h.drop_at(p2.center());
    h.run_steps(6);

    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Move to another page"));
    assert!(doc.page_images(0).is_empty(), "gone from page 1");
    assert_eq!(doc.added().iter().filter(|a| a.page == 1).count(), 1, "on page 2");
    let moved = doc.page_images(1);
    assert_eq!(moved.len(), 1, "drawn on page 2");
    let (mx, my) = ((moved[0].rect[0] + moved[0].rect[2]) / 2.0, (moved[0].rect[1] + moved[0].rect[3]) / 2.0);
    let (ox, oy) = ((before[0] + before[2]) / 2.0, (before[1] + before[3]) / 2.0);
    assert!((mx - ox).abs() < 2.0 && (my - oy).abs() < 2.0, "page 1's centre to page 2's: {before:?} -> {:?}", moved[0].rect);

    h.state_mut().execute("edit.undo");
    h.run_steps(4);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.page_images(0).len(), 1, "undo puts it back");
    assert!(doc.page_images(1).is_empty());
}

#[test]
fn a_page_picture_resized_by_a_corner_stretches_its_own_pixels() {
    let mut h = two_pages();
    let id = h.state().views[0].id;
    let c = h.state().views[0].page_screen_rect(0).expect("page 1 on screen").center();
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some(), "selected");
    // The picture's top right corner on screen, from its box.
    let r = h.state().session.get(id).unwrap().page_images(0)[0].rect;
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let info = h.state().session.get(id).unwrap().info.pages[0].clone();
    let at = |x: f64, y: f64| {
        egui::pos2(page.left() + (x as f32 / info.width) * page.width(), page.top() + (1.0 - y as f32 / info.height) * page.height())
    };
    let grip = at(r[2], r[3]);
    let to = grip + egui::vec2(60.0, -30.0);
    drag(&mut h, grip, to, true);
    let tex = h.state().views[0].image_drag_picture.as_ref().and_then(|(_, t)| t.as_ref()).map(|t| t.id()).expect("the picture's pixels are in");
    let drawn: Vec<egui::Rect> = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Mesh(m) if m.texture_id == tex => Some(m.calc_bounds()),
            _ => None,
        })
        .collect();
    let bottom_left = at(r[0], r[1]);
    assert!(
        drawn.iter().any(|b| (b.left_bottom() - bottom_left).length() < 2.0 && b.width() > (grip.x - bottom_left.x) + 20.0),
        "the picture alone grows from its fixed corner: {drawn:?}"
    );
    h.drop_at(to);
    h.run_steps(4);
}

/// Thai edition: Ctrl held as a page's own picture is dropped on its own page leaves it where it
/// was and adds a copy where it was dropped, as Ctrl does for added pictures and text; without
/// Ctrl the same drag moves it.
#[test]
fn a_page_picture_ctrl_dragged_on_its_own_page_is_copied_there() {
    let mut h = two_pages();
    let id = h.state().views[0].id;
    let p1 = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    let c = p1.center();
    let before = h.state().session.get(id).unwrap().page_images(0)[0].rect;
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some(), "selected");
    let to = c + egui::vec2(0.0, p1.height() / 4.0);
    let ctrl = egui::Modifiers::COMMAND;
    h.event(egui::Event::ModifiersChanged(ctrl));
    h.hover_at(c);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed: true, modifiers: ctrl });
    h.run_steps(1);
    for k in 1..=6 {
        h.hover_at(c + (to - c) * (k as f32 / 6.0));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: ctrl });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(4);
    let doc = h.state().session.get(id).unwrap();
    let images = doc.page_images(0);
    assert!(
        images.iter().any(|im| im.rect == before),
        "the original stays where it was: {before:?} in {:?}",
        images.iter().map(|i| i.rect).collect::<Vec<_>>()
    );
    assert_eq!(images.len(), 2, "and a copy is on the page");
    assert_eq!(doc.added().iter().filter(|a| a.page == 0).count(), 1, "the copy is an added picture on page 1");
    assert!(doc.added().iter().all(|a| a.page == 0), "nothing went to page 2");
    assert_eq!(doc.can_undo(), Some("Copy object"), "one step, named as a copy");
}

/// A key pressed with Ctrl (⌘ on a Mac) and released.
fn command_key(h: &mut Harness<'static, PdfCraftApp>, key: egui::Key) {
    let m = egui::Modifiers::COMMAND;
    h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m });
    h.event(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m });
    h.run_steps(3);
}

/// Width and height of a box.
fn size(r: [f64; 4]) -> (f64, f64) {
    (r[2] - r[0], r[3] - r[1])
}

/// Thai edition: a page's own picture copied with Ctrl+C and pasted with Ctrl+V (or Paste in the
/// page's menu) comes back the same picture at the same size, not a new one sized by its pixels;
/// after Ctrl+X too.
#[test]
fn a_page_picture_copied_and_pasted_keeps_its_size() {
    let mut h = two_pages();
    h.state_mut().system_clipboard = false;
    let id = h.state().views[0].id;
    let c = h.state().views[0].page_screen_rect(0).expect("page 1 on screen").center();
    let original = h.state().session.get(id).unwrap().page_images(0)[0].clone();
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some(), "selected");
    command_key(&mut h, egui::Key::C);
    command_key(&mut h, egui::Key::V);
    let doc = h.state().session.get(id).unwrap();
    let images = doc.page_images(0);
    assert_eq!(images.len(), 2, "pasted once");
    let pasted = images.iter().find(|im| im.rect != original.rect).expect("the paste");
    let ((w0, h0), (w1, h1)) = (size(original.rect), size(pasted.rect));
    assert!((w1 - w0).abs() < 0.01 && (h1 - h0).abs() < 0.01, "same size: {:?} vs {:?}", original.rect, pasted.rect);
    assert!((pasted.rect[0] - original.rect[0] - 12.0).abs() < 0.01, "a step right of it: {:?}", pasted.rect);
    assert_eq!(pasted.object, original.object, "the same picture, not a re-encoded copy");
    assert_eq!(h.state().quick_tool, pdfcraft_ui_egui::QuickTool::EditText, "still in Edit text & images");
    assert!(h.state().views[0].image_selection.is_some(), "with the paste selected");

    // Cut, then paste: the picture is gone from the page, so its pixels go back in, at its size.
    // Pressed by its top-left corner, which the paste (a step down and right) leaves uncovered.
    let c = {
        let doc = h.state().session.get(id).unwrap();
        let (page, im) = (&doc.info.pages[0], &doc.page_images(0)[0]);
        let screen = h.state().views[0].page_screen_rect(0).unwrap();
        let k = screen.width() as f64 / page.width as f64;
        egui::pos2(screen.left() + ((im.rect[0] + 4.0) * k) as f32, screen.top() + ((page.height as f64 - im.rect[3] + 4.0) * k) as f32)
    };
    let cut = h.state().session.get(id).unwrap().page_images(0)[0].rect;
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    command_key(&mut h, egui::Key::X);
    assert_eq!(h.state().session.get(id).unwrap().page_images(0).len(), 1, "cut");
    command_key(&mut h, egui::Key::V);
    let images = h.state().session.get(id).unwrap().page_images(0);
    assert_eq!(images.len(), 2, "pasted back");
    let (wc, hc) = size(cut);
    assert!(
        images.iter().all(|im| (size(im.rect).0 - wc).abs() < 0.01 && (size(im.rect).1 - hc).abs() < 0.01),
        "at its size: {:?}",
        images.iter().map(|i| i.rect).collect::<Vec<_>>()
    );

    // Paste in the page's menu: centred where it was asked, at the same size.
    assert!(h.state_mut().paste_image_at(1, [40.0, 30.0]));
    h.run_steps(2);
    let on_two = h.state().session.get(id).unwrap().page_images(1);
    let r = on_two.first().expect("pasted on page 2").rect;
    let (w2, h2) = size(r);
    assert!((w2 - w0).abs() < 0.01 && (h2 - h0).abs() < 0.01, "same size on page 2: {r:?}");
}

/// Thai edition: a page's own picture copied together with a text box pastes with it: the same
/// picture at its size, laid out with the box as they were, in one step.
#[test]
fn a_page_picture_and_a_text_box_copy_and_paste_together() {
    let mut h = two_pages();
    h.state_mut().system_clipboard = false;
    let id = h.state().views[0].id;
    let original = h.state().session.get(id).unwrap().page_images(0)[0].clone();
    let [x0, y0, x1, _] = original.rect;
    let t = pdfcraft_engine::AddedText { text: "Label".into(), rect: [x0, y0 - 6.0, x1, y0 - 1.0], size: 4.0, ..Default::default() };
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text: t }));
    h.run_steps(4);
    let screen = |h: &Harness<'static, PdfCraftApp>, x: f64, y: f64| {
        let doc = h.state().session.get(id).unwrap();
        let page = &doc.info.pages[0];
        let r = h.state().views[0].page_screen_rect(0).unwrap();
        let k = r.width() as f64 / page.width as f64;
        egui::pos2(r.left() + (x * k) as f32, r.top() + ((page.height as f64 - y) * k) as f32)
    };
    let picture = screen(&h, (x0 + x1) / 2.0, (original.rect[1] + original.rect[3]) / 2.0);
    let label = screen(&h, (x0 + x1) / 2.0, y0 - 3.5);
    for (p, m) in [(picture, egui::Modifiers::NONE), (label, egui::Modifiers::SHIFT)] {
        h.hover_at(p);
        h.run_steps(2);
        for pressed in [true, false] {
            h.event_modifiers(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: m }, m);
        }
        h.run_steps(4);
    }
    h.event(egui::Event::Copy);
    h.run_steps(2);
    assert!(h.state().object_clipboard.is_some(), "both copied");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::V);
    h.run_steps(4);
    let doc = h.state().session.get(id).unwrap();
    let images = doc.page_images(0);
    assert_eq!(images.len(), 2, "the picture pasted");
    let pasted = images.iter().find(|im| im.rect != original.rect).unwrap();
    assert!((size(pasted.rect).0 - size(original.rect).0).abs() < 0.01, "at its size");
    assert_eq!(doc.can_undo(), Some("Paste"));
}
