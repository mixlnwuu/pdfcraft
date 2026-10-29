use super::marks::{
    Background, Context, HeaderFooter, MarkKind, Watermark, add_picture_resource, bates_start, expand, matrix_number, n, picture, rgb, shrink_matrix,
};
use super::place::{
    END, Isolation, add_resources, begin, check, check_on_top, contents, isolation, page_fonts, page_list, place, place_inside_wrapper, stream_tag,
    undrawable, wrapper_role,
};
use super::place::{add_page_font_objects, page_text_width, text_lines_on_page};
use super::{Dict, Document, EditError, Object, added};

/// Add a header and footer to `pages` (0-based). With `replace`, existing headers and footers on
/// those pages are removed first (Acrobat's Replace Existing).
pub fn add_header_footer(doc: &mut Document, pages: &[usize], hf: &HeaderFooter, replace: bool, cx: &Context) -> Result<(), EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if hf.text.iter().all(|t| t.trim().is_empty()) {
        return Err(EditError::Invalid("type the header or footer text first".into()));
    }
    if let Some(c) = hf.first_undrawable() {
        return Err(undrawable(c));
    }
    if !(hf.font_size.is_finite() && hf.font_size > 0.0 && hf.font_size <= 200.0 && hf.margins.iter().all(|m| m.is_finite() && *m >= 0.0)) {
        return Err(EditError::Invalid("invalid font size or margins".into()));
    }
    // Page numbers count up from start_number, so the last selected page needs the most room.
    // Refuse the whole run before drawing anything: headers applied to some pages and then a
    // failed page number would leave the document half changed.
    let last_number = u64::from(hf.start_number)
        .checked_add(pages.len().saturating_sub(1) as u64)
        .ok_or_else(|| EditError::Invalid("the header page numbers are too large".into()))?;
    if last_number > u64::from(u32::MAX) {
        return Err(EditError::Invalid(format!(
            "the header start number {} leaves no room for {} pages (page numbers stop at {})",
            hf.start_number,
            pages.len(),
            u32::MAX
        )));
    }
    // The same goes for Bates numbers: the last selected page must still fit in a u64.
    let bates0 = hf.text.iter().find_map(|t| bates_start(t)).unwrap_or(1);
    if bates0.checked_add(pages.len().saturating_sub(1) as u64).is_none() {
        return Err(EditError::Invalid(format!(
            "the Bates start number {bates0} leaves no room for {} pages (Bates numbers stop at {})",
            pages.len(),
            u64::MAX
        )));
    }
    // Shrinking needs room on every page: refuse before anything is drawn or replaced.
    let shrinks = pages
        .iter()
        .map(|&i| hf.shrink.then(|| shrink_matrix(hf, all[i].display_size(doc), all[i].view_matrix(doc))).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    for &i in pages {
        check_on_top(doc, &all[i])?;
    }
    if replace {
        remove_marks(doc, pages, MarkKind::HeaderFooter)?;
    }
    let all = page_list(doc);
    let count = all.len();
    for (k, &i) in pages.iter().enumerate() {
        let page = &all[i];
        let (w, h) = page.display_size(doc);
        let number =
            u32::try_from(u64::from(hf.start_number) + k as u64).map_err(|_| EditError::Invalid("the header page numbers are too large".into()))?;
        let bates = bates0.checked_add(k as u64).ok_or_else(|| EditError::Invalid("the Bates numbers are too large".into()))?;
        let size = hf.font_size;
        let [top, bottom, left, right] = hf.margins;
        if !hf.plain() {
            let view = page.view_matrix(doc);
            let mut content = begin(MarkKind::HeaderFooter, "Header", view).into_bytes();
            let mut fonts = Dict::new();
            let mut underlines = String::new();
            for (slot, template) in hf.text.iter().enumerate() {
                if template.trim().is_empty() {
                    continue;
                }
                let text = expand(template, number, count, bates, cx);
                // Headers hang below the top margin; footers' last line sits on the bottom margin.
                let header_top = h - top + size * 0.15;
                let mut t = hf.item(&text, slot, w, header_top);
                if slot >= 3 {
                    let n_lines = added::lines(&t).len().max(1) as f64;
                    t = hf.item(&text, slot, w, bottom + (n_lines - 1.0) * size * 1.2 + size * 0.95);
                }
                // Font names already on the page or used by this mark: new ones never clash.
                let mut taken = page_fonts(doc, page);
                for (k, v) in fonts.iter() {
                    taken.set(k.clone(), v.clone());
                }
                let (ops, res) = added::text_ops(doc, &t, &taken)?;
                content.extend(ops);
                if let Some(Object::Dict(f)) = res.get(b"Font") {
                    for (k, v) in f.iter() {
                        fonts.set(k.clone(), v.clone());
                    }
                }
                if hf.underline {
                    let r = added::text_rect(&t);
                    for (li, line) in added::lines(&t).iter().enumerate() {
                        let tw = added::line_width(&t, line);
                        let x = match slot % 3 {
                            0 => r[0],
                            1 => r[0] + (r[2] - r[0] - tw) / 2.0,
                            _ => r[2] - tw,
                        };
                        let y = r[3] - (li as f64 * 1.2 + 0.95) * size;
                        underlines.push_str(&format!("{} {} {} {} re f\n", n(x), n(y - size * 0.15), n(tw), n((size * 0.06).max(0.4))));
                    }
                }
            }
            if !underlines.is_empty() {
                content.extend(format!("{}\n{underlines}", rgb(hf.color)).bytes());
            }
            content.extend_from_slice(END.as_bytes());
            add_resources(doc, page, None, None)?;
            let named: Vec<(Vec<u8>, Object)> = fonts.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            add_page_font_objects(doc, i, named)?;
            let page = &page_list(doc)[i];
            place(doc, page, MarkKind::HeaderFooter, content, false)?;
            if let Some(Some(m)) = shrinks.get(k) {
                let cm = format!(
                    "{} cm
",
                    m.map(matrix_number).join(" ")
                );
                let page = &page_list(doc)[i];
                place_inside_wrapper(doc, page, MarkKind::HeaderFooter.tag(), cm.into_bytes())?;
            }
            continue;
        }
        let mut body: Vec<u8> = format!("BT\n/PCHelv {} Tf\n{}\n", n(size), rgb(hf.color)).into_bytes();
        let mut underlines = String::new();
        let mut placed = Vec::new();
        for (slot, template) in hf.text.iter().enumerate() {
            if template.trim().is_empty() {
                continue;
            }
            let lines: Vec<String> = expand(template, number, count, bates, cx).lines().map(str::to_owned).collect();
            let header = slot < 3;
            let line_h = size * 1.2;
            for (li, line) in lines.iter().enumerate() {
                let tw = page_text_width(line, size);
                let x = match slot % 3 {
                    0 => left,
                    1 => (w - tw) / 2.0,
                    _ => w - right - tw,
                };
                // Headers hang below the top margin; footers sit on the bottom margin.
                let y = if header { h - top - size * 0.8 - li as f64 * line_h } else { bottom + (lines.len() - 1 - li) as f64 * line_h };
                placed.push((line.clone(), x, y));
                if hf.underline {
                    underlines.push_str(&format!("{} {} {} {} re f\n", n(x), n(y - size * 0.15), n(tw), n((size * 0.06).max(0.4))));
                }
            }
        }
        let view = page.view_matrix(doc);
        add_resources(doc, page, None, None)?;
        body.extend(text_lines_on_page(doc, i, &placed, size)?);
        body.extend_from_slice(b"ET\n");
        if !underlines.is_empty() {
            body.extend(format!("{}\n{underlines}", rgb(hf.color)).bytes());
        }
        let mut content = begin(MarkKind::HeaderFooter, "Header", view).into_bytes();
        content.extend(body);
        content.extend_from_slice(END.as_bytes());
        let page = &page_list(doc)[i];
        place(doc, page, MarkKind::HeaderFooter, content, false)?;
        if let Some(Some(m)) = shrinks.get(k) {
            let cm = format!("{} cm\n", m.map(matrix_number).join(" "));
            let page = &page_list(doc)[i];
            place_inside_wrapper(doc, page, MarkKind::HeaderFooter.tag(), cm.into_bytes())?;
        }
    }
    Ok(())
}

/// Add a text watermark to `pages`.
pub fn add_watermark(doc: &mut Document, pages: &[usize], wm: &Watermark, replace: bool) -> Result<(), EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let text = wm.text.trim();
    if text.is_empty() && wm.source.is_none() {
        return Err(EditError::Invalid("type the watermark text first".into()));
    }
    if let Some(c) = wm.first_undrawable() {
        return Err(undrawable(c));
    }
    if !(wm.opacity.is_finite()
        && wm.rotation.is_finite()
        && wm.font_size.is_finite()
        && wm.font_size >= 0.0
        && wm.offset.iter().all(|o| o.is_finite()))
    {
        return Err(EditError::Invalid("invalid watermark settings".into()));
    }
    if !wm.behind {
        for &i in pages {
            check_on_top(doc, &all[i])?;
        }
    }
    if replace {
        remove_marks(doc, pages, MarkKind::Watermark)?;
    }
    let opacity = wm.opacity.clamp(0.0, 1.0);
    let lines: Vec<&str> = text.lines().collect();
    for &i in pages {
        let page = page_list(doc)[i].clone();
        let (w, h) = page.display_size(doc);
        if let Some(src) = &wm.source {
            let mut content = begin(MarkKind::Watermark, "Watermark", page.view_matrix(doc));
            content.push_str(&format!("/PCGS{} gs\n", (opacity * 100.0).round() as i64));
            content.push_str(&picture(src, (w, h), wm.scale, wm.rotation, wm.offset));
            content.push_str(END);
            add_resources(doc, &page, Some(opacity), None)?;
            add_picture_resource(doc, &page_list(doc)[i].clone(), src)?;
            let page = &page_list(doc)[i];
            place(doc, page, MarkKind::Watermark, content.into_bytes(), wm.behind)?;
            continue;
        }
        if !wm.plain() {
            watermark_in_font(doc, i, &page, (w, h), wm, &lines, opacity)?;
            continue;
        }
        let widest = lines.iter().map(|l| page_text_width(l, 1.0)).fold(0.0, f64::max).max(0.01);
        let size = if wm.font_size > 0.0 { wm.font_size } else { ((w * w + h * h).sqrt() * 0.5 / widest).clamp(6.0, 300.0) };
        let (s, c) = wm.rotation.to_radians().sin_cos();
        let (cx, cy) = (w / 2.0 + wm.offset[0], h / 2.0 + wm.offset[1]);
        let mut content = begin(MarkKind::Watermark, "Watermark", page.view_matrix(doc)).into_bytes();
        content.extend(
            format!(
                "/PCGS{} gs\n{} {} {} {} {} {} cm\nBT\n/PCHelv {} Tf\n{}\n",
                (opacity * 100.0).round() as i64,
                n(c),
                n(s),
                n(-s),
                n(c),
                n(cx),
                n(cy),
                n(size),
                rgb(wm.color)
            )
            .bytes(),
        );
        let line_h = size * 1.15;
        let total = line_h * (lines.len() as f64 - 1.0);
        let placed: Vec<(String, f64, f64)> = lines
            .iter()
            .enumerate()
            .map(|(li, line)| (line.to_string(), -page_text_width(line, size) / 2.0, total / 2.0 - li as f64 * line_h - size * 0.35))
            .collect();
        add_resources(doc, &page, Some(opacity), None)?;
        content.extend(text_lines_on_page(doc, i, &placed, size)?);
        content.extend_from_slice(b"ET\n");
        content.extend_from_slice(END.as_bytes());
        let page = &page_list(doc)[i];
        place(doc, page, MarkKind::Watermark, content, wm.behind)?;
    }
    Ok(())
}

/// A text watermark in a chosen font on page `i` (`page`, `w` × `h` displayed), drawn as Add text
/// draws it (Thai edition): the lines centred on the page centre plus the offset, turned by the
/// rotation, at the font size or, for 0, about half the page diagonal.
#[allow(clippy::too_many_arguments)]
fn watermark_in_font(
    doc: &mut Document,
    i: usize,
    page: &pdfcraft_model::Page,
    (w, h): (f64, f64),
    wm: &Watermark,
    lines: &[&str],
    opacity: f64,
) -> Result<(), EditError> {
    let widest = lines.iter().map(|l| added::line_width(&wm.item(l, 1.0, 1.0e6), l)).fold(0.0, f64::max);
    if !(widest.is_finite() && widest > 0.0) {
        return Err(EditError::Invalid("invalid watermark settings".into()));
    }
    let size = if wm.font_size > 0.0 { wm.font_size.min(300.0) } else { ((w * w + h * h).sqrt() * 0.5 / widest).clamp(6.0, 300.0) };
    // Lines are 1.2 sizes apart (as Add text spaces them); the block's middle sits on the centre.
    let rows = lines.len().max(1) as f64;
    let top = (rows - 1.0) * 0.6 * size + 0.6 * size;
    // Wide enough that no line wraps: the widest line, a little room for rounding, one size.
    let mut t = wm.item(&lines.join("\n"), size, widest * size * 1.05 + size);
    t.rect = [t.rect[0], top - rows * 1.2 * size, t.rect[2], top];
    let (s, c) = wm.rotation.to_radians().sin_cos();
    let (cx, cy) = (w / 2.0 + wm.offset[0], h / 2.0 + wm.offset[1]);
    let mut content = begin(MarkKind::Watermark, "Watermark", page.view_matrix(doc)).into_bytes();
    content.extend(format!("/PCGS{} gs\n{} {} {} {} {} {} cm\n", (opacity * 100.0).round() as i64, n(c), n(s), n(-s), n(c), n(cx), n(cy)).bytes());
    let (ops, res) = added::text_ops(doc, &t, &page_fonts(doc, page))?;
    content.extend(ops);
    content.extend_from_slice(END.as_bytes());
    add_resources(doc, page, Some(opacity), None)?;
    let fonts: Vec<(Vec<u8>, Object)> = match res.get(b"Font") {
        Some(Object::Dict(f)) => f.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => Vec::new(),
    };
    add_page_font_objects(doc, i, fonts)?;
    let page = &page_list(doc)[i];
    place(doc, page, MarkKind::Watermark, content, wm.behind)
}

/// Add a solid-colour background behind the content of `pages`.
pub fn add_background(doc: &mut Document, pages: &[usize], bg: &Background, replace: bool) -> Result<(), EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    if replace {
        remove_marks(doc, pages, MarkKind::Background)?;
    }
    let opacity = if bg.opacity.is_finite() { bg.opacity.clamp(0.0, 1.0) } else { 1.0 };
    for &i in pages {
        let page = page_list(doc)[i].clone();
        let (w, h) = page.display_size(doc);
        let mut content = begin(MarkKind::Background, "Background", page.view_matrix(doc));
        match &bg.source {
            Some(src) => {
                content.push_str(&format!("/PCGS{} gs\n", (opacity * 100.0).round() as i64));
                content.push_str(&picture(src, (w, h), bg.scale, 0.0, [0.0; 2]));
            }
            None => content.push_str(&format!("/PCGS{} gs\n{}\n0 0 {} {} re f\n", (opacity * 100.0).round() as i64, rgb(bg.color), n(w), n(h))),
        }
        content.push_str(END);
        add_resources(doc, &page, Some(opacity), None)?;
        if let Some(src) = &bg.source {
            add_picture_resource(doc, &page_list(doc)[i].clone(), src)?;
        }
        let page = &page_list(doc)[i];
        place(doc, page, MarkKind::Background, content.into_bytes(), true)?;
    }
    Ok(())
}

/// Remove marks of `kind` from `pages`; returns how many were removed. Wrappers left with
/// nothing after them go too, restoring the original content list; a wrapper that content
/// after it still relies on (another tool's appended stream) stays.
pub fn remove_marks(doc: &mut Document, pages: &[usize], kind: MarkKind) -> Result<usize, EditError> {
    let all = page_list(doc);
    check(pages, all.len())?;
    let mut removed = 0;
    for &i in pages {
        let page = &all[i];
        let list = contents(doc, page)?;
        let before = list.len();
        let mut kept: Vec<Object> = list.into_iter().filter(|o| stream_tag(doc, o).as_deref() != Some(kind.tag())).collect();
        removed += before - kept.len();
        // Strip only wrapper pairs whose close ends the list (after stripping the pairs after
        // it): nothing comes after them to need the state they restore. Pairs come from the
        // operators; for content that can't be read that precisely, from the wrapper streams
        // alone, so updating a mark there doesn't nest another wrapper each time.
        if before != kept.len() {
            let wrappers = match isolation(doc, &kept) {
                Isolation::Clean { wrappers, .. } => wrappers,
                Isolation::Unknown { .. } | Isolation::TooDeep => {
                    let mut opens = Vec::new();
                    let mut pairs = Vec::new();
                    for (i, o) in kept.iter().enumerate() {
                        match wrapper_role(doc, o) {
                            Some(true) => opens.push(i),
                            Some(false) => pairs.extend(opens.pop().map(|open| (open, i))),
                            None => {}
                        }
                    }
                    pairs
                }
            };
            let pairs: std::collections::HashMap<usize, usize> = wrappers.into_iter().map(|(open, close)| (close, open)).collect();
            let mut discarded = std::collections::HashSet::new();
            let mut end = kept.len();
            while let Some(close) = end.checked_sub(1) {
                if discarded.contains(&close) {
                    end = close;
                } else if let Some(&open) = pairs.get(&close) {
                    discarded.insert(open);
                    discarded.insert(close);
                    end = close;
                } else {
                    break;
                }
            }
            kept = kept.into_iter().enumerate().filter(|(i, _)| !discarded.contains(i)).map(|(_, o)| o).collect();
        }
        if before != kept.len() {
            doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(kept)))?;
        }
    }
    Ok(removed)
}

/// Which kinds of marks the document has (for enabling Update/Remove).
pub fn marks_present(doc: &Document) -> Vec<MarkKind> {
    added::scan_markers(doc).marks
}
