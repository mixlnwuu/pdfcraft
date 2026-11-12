//! Print as image (Acrobat's Advanced Print Setup ▸ Print As Image): every page
//! sent to the printer as a picture at a chosen resolution, for printers that skip text,
//! shading or transparency. The pages are rendered the way the viewer shows them (reading
//! colours off, the print's own colours) with only what the print's content choice keeps, made
//! into an image-only document, and that document is imposed on the paper like any other. The
//! print is no longer searchable text.

use pdfcraft_cos::{SaveOptions, write_incremental};
use pdfcraft_create::ImageResolution;
use pdfcraft_print::{Content, Raster};
use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderRequest};

use crate::export::encode_png;
use crate::{Document, guard, print_layer_states};

/// `doc`'s pages `pages` (0-based, each once, in the order given) as an image-only PDF, one
/// page per entry, the size of the source page as displayed, drawing what `content` prints.
/// Pages too large for the renderer at `raster.dpi` render at the largest resolution it allows.
pub fn rasterize(doc: &Document, pages: &[usize], raster: &Raster, content: Content) -> Result<Vec<u8>, String> {
    let dpi = raster.dpi.clamp(72.0, 600.0);
    let src = doc.export_source();
    let mut config = src.config.clone();
    // The copy below holds only what prints, so the viewer's own comment hiding plays no part.
    config.hide_comments = false;
    let (printed, layers) = guard(|| -> Result<(Vec<u8>, Option<Vec<pdfcraft_render::Layer>>), String> {
        let mut cos = pdfcraft_cos::Document::open_with_password(src.bytes.clone(), config.password.as_deref()).map_err(|e| e.to_string())?;
        pdfcraft_print::keep_printed(&mut cos, content);
        // The layers as the vector print sets them: as shown, then the Print event's auto state.
        let layers = print_layer_states(&cos, &doc.info.layers);
        Ok((write_incremental(&cos, &SaveOptions::default()).map_err(|e| e.to_string())?, layers))
    })??;
    if let Some(layers) = layers {
        config.layers = std::sync::Arc::new(layers.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect());
    }
    let mut renderer = PageRenderer::new(std::sync::Arc::new(printed), config);
    let mut images: Vec<(String, Vec<u8>)> = Vec::with_capacity(pages.len());
    let mut used: Option<f64> = None;
    for &page in pages {
        let (w, h) = src.sizes.get(page).copied().ok_or_else(|| format!("page {} does not exist", page + 1))?;
        let scale = pdfcraft_render::effective_scale(w, h, (dpi / 72.0) as f32);
        let page_dpi = f64::from(scale) * 72.0;
        // Every page of the image document shares one resolution, so its pages keep the
        // source's sizes; the lowest a page needed is used for all.
        used = Some(used.map_or(page_dpi, |u: f64| u.min(page_dpi)));
        let r = renderer.render(RenderRequest { page, scale, tag: PRINT_TAG, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("page {}: {e}", page + 1));
        }
        let mut rgba = r.rgba.to_vec();
        over_white(&mut rgba, raster.grayscale);
        images.push((format!("page-{}.png", page + 1), encode_png(r.width, r.height, &rgba)?));
    }
    let dpi = used.unwrap_or(dpi);
    let doc = pdfcraft_create::from_images_with_resolution(&images, ImageResolution::Dpi(dpi)).map_err(|e| e.to_string())?;
    pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())
}

/// Premultiplied RGBA composed over white paper (opaque), in grey when asked (Rec. 601 luma).
fn over_white(rgba: &mut [u8], grayscale: bool) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let gap = 255 - u32::from(px[3]);
        let (r, g, b) = ((u32::from(px[0]) + gap).min(255), (u32::from(px[1]) + gap).min(255), (u32::from(px[2]) + gap).min(255));
        let (r, g, b) = if grayscale {
            let y = ((299 * r + 587 * g + 114 * b) / 1000).min(255);
            (y, y, y)
        } else {
            (r, g, b)
        };
        px[0] = r as u8;
        px[1] = g as u8;
        px[2] = b as u8;
        px[3] = 255;
    }
}

/// Most sheets [`sheets_as_tiff`] draws: a print job longer than this is refused rather than
/// held as pictures in memory.
const MAX_PICTURE_SHEETS: usize = 2_000;

/// The print-ready `pdf`'s sheets as a multi-page TIFF (RGB, LZW), one page per sheet at `dpi`
/// (lowered for a sheet too large for the renderer), each page's resolution recorded so it keeps
/// the sheet's size, in grey when asked. Thai edition: what the Windows spooler gets for a job
/// printed as pictures (`craft_print::submit_pictures_with_progress`), so the pictures arrive at
/// their full resolution instead of `Windows.Data.Pdf`'s 96 dpi.
pub fn sheets_as_tiff(pdf: &[u8], dpi: f64, grayscale: bool) -> Result<Vec<u8>, String> {
    use tiff::encoder::{Compression, Rational, TiffEncoder, colortype::RGB8};
    use tiff::tags::{ResolutionUnit, Tag};
    let err = |e: tiff::TiffError| e.to_string();
    let bytes = std::sync::Arc::new(pdf.to_vec());
    let info = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| e.to_string())?;
    if info.pages.is_empty() || info.pages.len() > MAX_PICTURE_SHEETS {
        return Err(format!("a print job of {} sheets can't be drawn as pictures", info.pages.len()));
    }
    let dpi = if dpi.is_finite() { dpi.clamp(72.0, 600.0) } else { 300.0 };
    let mut renderer = PageRenderer::new(bytes, pdfcraft_render::RenderConfig::default());
    let mut out = std::io::Cursor::new(Vec::new());
    let mut tiff = TiffEncoder::new(&mut out).map_err(err)?.with_compression(Compression::Lzw);
    for (page, p) in info.pages.iter().enumerate() {
        let scale = pdfcraft_render::effective_scale(p.width, p.height, (dpi / 72.0) as f32);
        let r = renderer.render(RenderRequest { page, scale, tag: PRINT_TAG, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("sheet {}: {e}", page + 1));
        }
        let mut rgba = r.rgba.to_vec();
        over_white(&mut rgba, grayscale);
        let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        // The resolution the sheet was drawn at, so the picture keeps the sheet's width.
        let page_dpi = f64::from(r.width) * 72.0 / f64::from(p.width.max(1.0));
        let resolution = Rational { n: (page_dpi * 100.0).round() as u32, d: 100 };
        let mut image = tiff.new_image::<RGB8>(r.width, r.height).map_err(err)?;
        let tags = image.encoder();
        tags.write_tag(Tag::ResolutionUnit, ResolutionUnit::Inch).map_err(err)?;
        tags.write_tag(Tag::XResolution, resolution.clone()).map_err(err)?;
        tags.write_tag(Tag::YResolution, resolution).map_err(err)?;
        image.write_data(&rgb).map_err(err)?;
    }
    // The encoder borrows `out`; it writes each page as it is added, so nothing is pending here.
    let _ = tiff;
    Ok(out.into_inner())
}
