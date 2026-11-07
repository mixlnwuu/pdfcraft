//! Thai edition: renders every app-icon file in `assets/app-icon` (except the PDF document icon)
//! from the edition's cat, on a rounded red tile.
//!
//! Sizes from 40 px up show the cat's head and hood as drawn. At 32 px and below the head sits a
//! little smaller, so the red frames its ears, and the greys are lifted, so the hood stays apart
//! from the face instead of turning into one dark blot.
use std::path::{Path, PathBuf};
use tiny_skia::{Color, ColorU8, FillRule, FilterQuality, Mask, Paint, Path as SkPath, PathBuilder, Pixmap, PixmapPaint, Transform};

/// The tile's colour (chosen by the owner, 2026-11-05).
const RED: (u8, u8, u8) = (0xE5, 0x39, 0x35);
/// Head and hood, in the source picture's pixels: centre and side of the square shown.
const HEAD: (f32, f32, f32) = (452.0, 412.0, 640.0);
const HEAD_SMALL: (f32, f32, f32) = (452.0, 412.0, 700.0);
/// Gamma for the greys at small sizes (below 1 lifts them).
const LIFT_SMALL: f32 = 0.6;
/// Largest size drawn the small way.
const SMALL_MAX: u32 = 32;
/// Corner radius as a share of the side: the upstream master's 112 on 512.
const CORNER: f32 = 112.0 / 512.0;
const ID: &str = "ai.storyteller.pdfcraft";

type Result<T> = std::result::Result<T, String>;

fn main() {
    if let Err(e) = run() {
        eprintln!("thai-icons: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let cat_path = root.join("thai/brand/thai-edition-cat-original.png");
    let cat = Pixmap::load_png(&cat_path).map_err(|e| format!("{}: {e}", cat_path.display()))?;
    let lifted = lift(&cat, LIFT_SMALL);
    let tile = |size: u32| if size <= SMALL_MAX { render(&lifted, size, HEAD_SMALL) } else { render(&cat, size, HEAD) };
    let dir = root.join("assets/app-icon");

    // Windows .ico: PNG entries, 16-256 px (exe, Start menu, taskbar, the MSI, Add/Remove Programs).
    let mut ico = Vec::new();
    for s in [16, 20, 24, 32, 40, 48, 64, 128, 256] {
        ico.push(png(&tile(s)?)?);
    }
    write(&dir.join("pdfcraft.ico"), &ico_file(&ico)?)?;

    // Linux hicolor theme; the 256 px one is also the runtime window icon on Windows and Linux.
    for s in [16, 24, 32, 48, 64, 128, 256, 512] {
        write(&dir.join(format!("hicolor/{s}x{s}/apps/{ID}.png")), &png(&tile(s)?)?)?;
    }

    // macOS: Apple's grid, an 824 px body centred on a transparent 1024 canvas.
    let mac = |size: u32| -> Result<Pixmap> {
        let body = tile(((size as f32) * 824.0 / 1024.0).round() as u32)?;
        let mut out = Pixmap::new(size, size).ok_or("canvas")?;
        let off = i32::try_from(size.saturating_sub(body.width()) / 2).map_err(|_| "offset")?;
        out.draw_pixmap(off, off, body.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        Ok(out)
    };
    write(&dir.join("pdfcraft-1024.png"), &png(&mac(1024)?)?)?;
    let mut icns = Vec::new();
    for (kind, s) in [(b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024)] {
        icns.push((*kind, png(&mac(s)?)?));
    }
    write(&dir.join("pdfcraft.icns"), &icns_file(&icns)?)?;

    // The SVGs wrap a rendered PNG, so places that want a vector file (README, Linux scalable,
    // the macOS disk image background) still find one.
    let svg = |size: u32| -> Result<Vec<u8>> {
        let data = base64(&png(&tile(size)?)?);
        Ok(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 512 512\"><image width=\"512\" height=\"512\" xlink:href=\"data:image/png;base64,{data}\"/></svg>\n"
        )
        .into_bytes())
    };
    let master = svg(512)?;
    write(&dir.join("pdfcraft.svg"), &master)?;
    write(&dir.join(format!("hicolor/scalable/apps/{ID}.svg")), &master)?;
    write(&dir.join("pdfcraft-small.svg"), &svg(256)?)?;
    Ok(())
}

/// The square of `cat` given by `(cx, cy, side)` on a rounded red tile `size` px wide, drawn at
/// four times the size and scaled down.
fn render(cat: &Pixmap, size: u32, (cx, cy, side): (f32, f32, f32)) -> Result<Pixmap> {
    const SS: u32 = 4;
    let big = size.checked_mul(SS).ok_or("size")?;
    let b = big as f32;
    let mut pm = Pixmap::new(big, big).ok_or("canvas")?;
    let path = rounded(b, b * CORNER).ok_or("tile path")?;
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(RED.0, RED.1, RED.2, 255));
    paint.anti_alias = true;
    pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    let mut mask = Mask::new(big, big).ok_or("mask")?;
    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    let k = b / side;
    let t = Transform::from_translate(-(cx - side / 2.0), -(cy - side / 2.0)).post_scale(k, k);
    let smooth = PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() };
    pm.draw_pixmap(0, 0, cat.as_ref(), &smooth, t, Some(&mask));
    let mut out = Pixmap::new(size, size).ok_or("canvas")?;
    let k = 1.0 / SS as f32;
    out.draw_pixmap(0, 0, pm.as_ref(), &smooth, Transform::from_scale(k, k), None);
    Ok(out)
}

fn rounded(s: f32, r: f32) -> Option<SkPath> {
    let mut pb = PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(s - r, 0.0);
    pb.quad_to(s, 0.0, s, r);
    pb.line_to(s, s - r);
    pb.quad_to(s, s, s - r, s);
    pb.line_to(r, s);
    pb.quad_to(0.0, s, 0.0, s - r);
    pb.line_to(0.0, r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}

/// `cat` with every colour channel raised to `gamma` (alpha kept).
fn lift(cat: &Pixmap, gamma: f32) -> Pixmap {
    let mut out = cat.clone();
    let f = |v: u8| ((f32::from(v) / 255.0).powf(gamma) * 255.0).round().clamp(0.0, 255.0) as u8;
    for p in out.pixels_mut() {
        let c = p.demultiply();
        *p = ColorU8::from_rgba(f(c.red()), f(c.green()), f(c.blue()), c.alpha()).premultiply();
    }
    out
}

fn png(p: &Pixmap) -> Result<Vec<u8>> {
    p.encode_png().map_err(|e| e.to_string())
}

/// An .ico of PNG entries (sizes read from each PNG's header; 256 is written as 0).
fn ico_file(pngs: &[Vec<u8>]) -> Result<Vec<u8>> {
    let n = u16::try_from(pngs.len()).map_err(|_| "too many entries")?;
    let mut head = vec![0, 0, 1, 0];
    head.extend_from_slice(&n.to_le_bytes());
    let mut data = Vec::new();
    let mut offset = 6 + 16 * u32::from(n);
    for b in pngs {
        let dim = |at: usize| b.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).map(u32::from_be_bytes).ok_or("not a PNG");
        let (w, h) = (dim(16)?, dim(20)?);
        let len = u32::try_from(b.len()).map_err(|_| "entry too large")?;
        head.extend_from_slice(&[(w % 256) as u8, (h % 256) as u8, 0, 0]);
        head.extend_from_slice(&1u16.to_le_bytes());
        head.extend_from_slice(&32u16.to_le_bytes());
        head.extend_from_slice(&len.to_le_bytes());
        head.extend_from_slice(&offset.to_le_bytes());
        data.extend_from_slice(b);
        offset = offset.checked_add(len).ok_or("ico too large")?;
    }
    head.extend_from_slice(&data);
    Ok(head)
}

/// An .icns of PNG entries.
fn icns_file(entries: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for (kind, b) in entries {
        let len = u32::try_from(b.len() + 8).map_err(|_| "entry too large")?;
        body.extend_from_slice(kind);
        body.extend_from_slice(&len.to_be_bytes());
        body.extend_from_slice(b);
    }
    let total = u32::try_from(body.len() + 8).map_err(|_| "icns too large")?;
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let byte = |i: usize| u32::from(c.get(i).copied().unwrap_or(0));
        let n = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for i in 0..4 {
            if i <= c.len() {
                s.push(char::from(A[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}
