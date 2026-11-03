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
