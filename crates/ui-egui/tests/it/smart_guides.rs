//! Thai edition: smart guides as in Photoshop. A box moved in Edit text & images snaps onto the
//! edges and centres of the other things on the page and draws a guide to them.
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// The guides' colour (`content_ui::GUIDE`).
const GUIDE: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x2E, 0xC8);

/// An empty 300 × 300 point page.
fn blank() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << >> /Contents 4 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

fn added_rects(h: &Harness<'static, PdfCraftApp>) -> Vec<[f64; 4]> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().added().iter().map(|a| a.content.rect()).collect()
}

#[test]
fn a_box_moved_near_another_lines_up_with_it_and_shows_a_guide() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("blank.pdf", None, blank()).unwrap();
        app.set_option("zoom", "200").unwrap();
        app
    });
    crate::support::settle(&mut h);
    for rect in [[40.0, 200.0, 140.0, 220.0], [60.0, 100.0, 160.0, 120.0]] {
        let text = pdfcraft_engine::AddedText { text: "Box".into(), rect, size: 12.0, ..Default::default() };
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text }));
    }
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let at = |x: f64, y: f64| egui::pos2(page.left() + x as f32 * k, page.top() + (300.0 - y as f32) * k);
    let [b0, b1, b2, b3] = added_rects(&h)[1];
    let c = at((b0 + b2) / 2.0, (b1 + b3) / 2.0);
    // Select the lower box, then drag it left by 18 points: its left edge comes 2 points short of
    // the upper box's.
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    let to = c - egui::vec2(18.0 * k, 0.0);
    h.drag_at(c);
    h.run_steps(1);
    for s in 1..=6 {
        h.hover_at(c + (to - c) * (s as f32 / 6.0));
        h.run_steps(1);
    }
    let guides = h.output().shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == GUIDE)).count();
    assert!(guides > 0, "a guide shows the boxes line up");
    h.drop_at(to);
    h.run_steps(4);
    let rects = added_rects(&h);
    assert!((rects[1][0] - rects[0][0]).abs() < 0.01, "the lower box's left edge lands on the upper's: {rects:?}");

    // Alt held moves freely.
    let [b0, b1, b2, b3] = rects[1];
    let c = at((b0 + b2) / 2.0, (b1 + b3) / 2.0);
    let to = c + egui::vec2(2.0 * k, 0.0);
    let alt = egui::Modifiers::ALT;
    h.event(egui::Event::ModifiersChanged(alt));
    h.hover_at(c);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: c, button: egui::PointerButton::Primary, pressed: true, modifiers: alt });
    h.run_steps(1);
    for s in 1..=6 {
        h.hover_at(c + (to - c) * (s as f32 / 6.0));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: alt });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(4);
    let moved = added_rects(&h)[1][0];
    assert!((moved - (rects[1][0] + 2.0)).abs() < 0.3, "with Alt it lands 2 points right: {moved}");
}

/// Thai edition: View ▸ Rulers and View ▸ Grid show rulers along the document and a grid over
/// the page; both are remembered, and neither touches the document.
#[test]
fn rulers_and_grid_show_from_the_view_menu() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("blank.pdf", None, blank()).unwrap();
        app.set_option("zoom", "200").unwrap();
        app
    });
    crate::support::settle(&mut h);
    let lines = |h: &Harness<'static, PdfCraftApp>| h.output().shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::LineSegment { .. })).count();
    let label = |h: &Harness<'static, PdfCraftApp>, text: &str| {
        h.output().shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == text))
    };
    let before = lines(&h);
    assert!(!label(&h, "in"), "no rulers yet");
    assert!(h.state_mut().execute("view.grid"));
    assert!(h.state_mut().execute("view.rulers"));
    h.run_steps(3);
    assert!(h.state().show_grid && h.state().show_rulers);
    // A 300 point page at 200 %: a quarter-inch grid is 50 lines across and down.
    assert!(lines(&h) > before + 50, "the grid's lines: {} before, {} after", before, lines(&h));
    assert!(label(&h, "in"), "the rulers say their units");
    assert!(label(&h, "1"), "and number the inches");
    let bytes = h.state().session.get(h.state().views[0].id).unwrap().can_undo().map(str::to_owned);
    assert_eq!(bytes, None, "the document is untouched");
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(2);
    assert!(!h.state().show_grid);
}

/// Thai edition: View ▸ Smart Guides and View ▸ Snap to Guides turn the guides and the snapping
/// off for whoever doesn't want them: the same drag that lined the box up now moves it as
/// dragged, with no guide drawn; both are remembered.
#[test]
fn smart_guides_and_snapping_turn_off_from_the_view_menu() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("blank.pdf", None, blank()).unwrap();
        app.set_option("zoom", "200").unwrap();
        app
    });
    crate::support::settle(&mut h);
    for rect in [[40.0, 200.0, 140.0, 220.0], [60.0, 100.0, 160.0, 120.0]] {
        let text = pdfcraft_engine::AddedText { text: "Box".into(), rect, size: 12.0, ..Default::default() };
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text }));
    }
    assert!(h.state().show_smart_guides && h.state().snap_to_guides, "on by default");
    assert!(h.state_mut().execute("view.smart_guides"));
    assert!(h.state_mut().execute("view.snap"));
    assert!(!h.state().show_smart_guides && !h.state().snap_to_guides);
    let mut restored = PdfCraftApp::new();
    restored.restore(&h.state().persist());
    assert!(!restored.show_smart_guides && !restored.snap_to_guides, "remembered");
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let at = |x: f64, y: f64| egui::pos2(page.left() + x as f32 * k, page.top() + (300.0 - y as f32) * k);
    let before = added_rects(&h)[1];
    let c = at((before[0] + before[2]) / 2.0, (before[1] + before[3]) / 2.0);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    let to = c - egui::vec2(18.0 * k, 0.0);
    h.drag_at(c);
    h.run_steps(1);
    for s in 1..=6 {
        h.hover_at(c + (to - c) * (s as f32 / 6.0));
        h.run_steps(1);
    }
    let guides = h.output().shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == GUIDE)).count();
    assert_eq!(guides, 0, "no guide drawn");
    h.drop_at(to);
    h.run_steps(4);
    let moved = added_rects(&h)[1][0];
    assert!((moved - (before[0] - 18.0)).abs() < 0.3, "moved as dragged, 2 points short of the other box: {moved}");
}

/// Thai edition: resizing snaps too. A text box's corner dragged to 2 points short of another
/// box's right edge lands on it and shows a guide; with Alt it resizes as dragged.
#[test]
fn a_box_resized_by_a_corner_snaps_its_edge_to_another_box() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("blank.pdf", None, blank()).unwrap();
        app.set_option("zoom", "200").unwrap();
        app
    });
    crate::support::settle(&mut h);
    for rect in [[40.0, 200.0, 140.0, 220.0], [60.0, 100.0, 160.0, 120.0]] {
        let text = pdfcraft_engine::AddedText { text: "Box".into(), rect, size: 12.0, ..Default::default() };
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text }));
    }
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let at = |x: f64, y: f64| egui::pos2(page.left() + x as f32 * k, page.top() + (300.0 - y as f32) * k);
    let resize = |h: &mut Harness<'static, PdfCraftApp>, by: f32, modifiers: egui::Modifiers| -> Vec<[f64; 4]> {
        let [b0, b1, b2, b3] = added_rects(h)[1];
        // Select the lower box with a click, then drag its bottom-right corner left.
        let c = at((b0 + b2) / 2.0, (b1 + b3) / 2.0);
        h.hover_at(c);
        h.run_steps(1);
        h.drag_at(c);
        h.run_steps(1);
        h.drop_at(c);
        h.run_steps(2);
        let corner = at(b2, b1);
        let to = corner - egui::vec2(by * k, 0.0);
        h.event(egui::Event::ModifiersChanged(modifiers));
        h.hover_at(corner);
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: corner, button: egui::PointerButton::Primary, pressed: true, modifiers });
        h.run_steps(1);
        for s in 1..=6 {
            h.hover_at(corner + (to - corner) * (s as f32 / 6.0));
            h.run_steps(1);
        }
        let guides =
            h.output().shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == GUIDE)).count();
        h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
        h.run_steps(1);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.run_steps(4);
        let rects = added_rects(h);
        assert_eq!(guides > 0, modifiers.is_none(), "a guide shows while snapping, none with Alt: {guides}");
        rects
    };
    // The right edge, 160, dragged 18 left: 2 short of the upper box's right edge, 140.
    let rects = resize(&mut h, 18.0, egui::Modifiers::NONE);
    assert!((rects[1][2] - 140.0).abs() < 0.01, "the right edge lands on the upper box's: {rects:?}");
    assert!((rects[1][0] - 60.0).abs() < 0.01, "the left edge stays: {rects:?}");
    // Alt held resizes freely: 2 more points left lands at 138.
    let rects = resize(&mut h, 2.0, egui::Modifiers::ALT);
    assert!((rects[1][2] - 138.0).abs() < 0.3, "with Alt it resizes as dragged: {rects:?}");
}

/// Thai edition: the page's centre lines are green, as before the smart guides, so they stand
/// apart from the magenta lines to other boxes.
#[test]
fn the_page_centre_guide_is_green_and_box_guides_stay_magenta() {
    const CENTRE: egui::Color32 = egui::Color32::from_rgb(0x2E, 0xB8, 0x5C);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("blank.pdf", None, blank()).unwrap();
        app.set_option("zoom", "200").unwrap();
        app
    });
    crate::support::settle(&mut h);
    // A box whose centre is at x = 90, and another lower down at x 40..140.
    for rect in [[40.0, 200.0, 140.0, 220.0], [40.0, 60.0, 140.0, 80.0]] {
        let text = pdfcraft_engine::AddedText { text: "Box".into(), rect, size: 12.0, ..Default::default() };
        assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text }));
    }
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let at = |x: f64, y: f64| egui::pos2(page.left() + x as f32 * k, page.top() + (300.0 - y as f32) * k);
    let lines = |h: &Harness<'static, PdfCraftApp>, colour: egui::Color32| -> Vec<[egui::Pos2; 2]> {
        h.output()
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::LineSegment { points, stroke } if stroke.color == colour => Some(*points),
                _ => None,
            })
            .collect()
    };
    let c = at(90.0, 210.0);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    // Dragged 59 points right: 1 short of the page's centre line (x = 150), which it snaps onto.
    let to = c + egui::vec2(59.0 * k, 0.0);
    h.drag_at(c);
    h.run_steps(1);
    for s in 1..=6 {
        h.hover_at(c + (to - c) * (s as f32 / 6.0));
        h.run_steps(1);
    }
    let green = lines(&h, CENTRE);
    assert!(
        green.iter().any(|[a, b]| (a.x - page.center().x).abs() < 0.6 && (b.x - a.x).abs() < 0.1 && (b.y - a.y).abs() >= page.height() - 1.0),
        "the vertical centre line, across the page, is green: {green:?}"
    );
    assert!(lines(&h, GUIDE).iter().all(|[a, b]| (a.x - page.center().x).abs() > 0.6 || (b.x - a.x).abs() > 0.1), "and not magenta");
    h.drop_at(to);
    h.run_steps(4);
    // Lined up with the other box's left edge instead: a magenta line, no green one.
    let r = added_rects(&h)[0];
    let c = at((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let to = c - egui::vec2((r[0] - 40.0 - 2.0) as f32 * k, 0.0);
    h.drag_at(c);
    h.run_steps(1);
    for s in 1..=6 {
        h.hover_at(c + (to - c) * (s as f32 / 6.0));
        h.run_steps(1);
    }
    assert!(!lines(&h, GUIDE).is_empty(), "a box-to-box guide is magenta");
    assert!(lines(&h, CENTRE).is_empty(), "no green line off the centre");
    h.drop_at(to);
    h.run_steps(2);
}

/// A 300 × 300 point page covered by a scan (a grey picture) drawn a point right of centre, as
/// scans often are.
fn scanned() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>",
        "<< /Length 32 >>\nstream\nq 300 0 0 300 2 0 cm /Im0 Do Q\n\nendstream",
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{e0}\nendstream",
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

/// Thai edition: on a scanned page the box snaps onto the page's centre line, not onto the
/// centre of the scan beneath it (a point off), which drew a magenta guide instead of the green
/// one and kept the box off centre.
#[test]
fn on_a_scanned_page_a_box_snaps_to_the_page_centre_not_the_scans() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scanned()).unwrap();
        app.set_option("zoom", "200").unwrap();
        app
    });
    crate::support::settle(&mut h);
    let text = pdfcraft_engine::AddedText { text: "Box".into(), rect: [40.0, 200.0, 140.0, 220.0], size: 12.0, ..Default::default() };
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::AddText { page: 0, text }));
    assert!(h.state_mut().execute("edit.edit_text"));
    crate::support::settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = page.width() / 300.0;
    let at = |x: f64, y: f64| egui::pos2(page.left() + x as f32 * k, page.top() + (300.0 - y as f32) * k);
    let c = at(90.0, 210.0);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    // Its centre (x 90) dragged to x 151.5: half a point past the scan's centre (151), 1.5 past
    // the page's (150).
    let to = c + egui::vec2(61.5 * k, 0.0);
    h.drag_at(c);
    h.run_steps(1);
    for s in 1..=6 {
        h.hover_at(c + (to - c) * (s as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
    let r = added_rects(&h)[0];
    assert!(((r[0] + r[2]) / 2.0 - 150.0).abs() < 0.05, "centred on the page: {r:?}");
}
