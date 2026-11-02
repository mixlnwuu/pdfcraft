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
