use std::collections::HashMap;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgb, RgbImage, RgbaImage};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint,
    Stroke, StrokeDash, Transform,
};

use crate::draw_command::{DrawCommand, KillFeedEntry, RenderTarget, ShipVisibility};

const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

fn load_font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_DATA).expect("failed to load embedded font")
}

// ── Pixmap conversion helpers ──────────────────────────────────────────────

/// Convert an RGB image (no alpha) to a tiny-skia Pixmap (opaque RGBA, premultiplied).
fn rgb_to_pixmap(img: &RgbImage) -> Pixmap {
    let w = img.width();
    let h = img.height();
    let mut pm = Pixmap::new(w, h).expect("failed to create pixmap");
    let data = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y).0;
            let idx = (y * w + x) as usize * 4;
            data[idx] = px[0];
            data[idx + 1] = px[1];
            data[idx + 2] = px[2];
            data[idx + 3] = 255;
        }
    }
    pm
}

/// Convert a tiny-skia Pixmap (premultiplied RGBA) back to an RGB image.
fn pixmap_to_rgb(pm: &Pixmap) -> RgbImage {
    let w = pm.width();
    let h = pm.height();
    let data = pm.data();
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize * 4;
            let a = data[idx + 3] as f32 / 255.0;
            // Unpremultiply alpha
            let (r, g, b) = if a > 0.001 {
                (
                    (data[idx] as f32 / a).min(255.0) as u8,
                    (data[idx + 1] as f32 / a).min(255.0) as u8,
                    (data[idx + 2] as f32 / a).min(255.0) as u8,
                )
            } else {
                (0, 0, 0)
            };
            img.put_pixel(x, y, Rgb([r, g, b]));
        }
    }
    img
}

/// Convert an RGBA image to a tiny-skia Pixmap (premultiplied alpha).
fn rgba_to_pixmap(img: &RgbaImage) -> Pixmap {
    let w = img.width();
    let h = img.height();
    let mut pm = Pixmap::new(w, h).expect("failed to create pixmap");
    let data = pm.data_mut();
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y).0;
            let idx = (y * w + x) as usize * 4;
            let a = px[3] as f32 / 255.0;
            // Premultiply
            data[idx] = (px[0] as f32 * a) as u8;
            data[idx + 1] = (px[1] as f32 * a) as u8;
            data[idx + 2] = (px[2] as f32 * a) as u8;
            data[idx + 3] = px[3];
        }
    }
    pm
}

// ── Paint helpers ──────────────────────────────────────────────────────────

/// Create a solid-color paint with the given RGBA values.
fn solid_paint(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

/// Create a solid-color paint from an [u8; 3] array with alpha.
fn color_paint(color: [u8; 3], alpha: f32) -> Paint<'static> {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    solid_paint(color[0], color[1], color[2], a)
}

// ── Text rendering directly onto Pixmap ────────────────────────────────────

/// Draw anti-aliased text onto a Pixmap at (x, y) with the given color.
///
/// Uses ab_glyph's per-pixel coverage callback for proper anti-aliasing.
/// Coordinates are in pixel space (x = left edge, y = top edge of text).
fn draw_text(
    pm: &mut Pixmap,
    color: [u8; 3],
    x: i32,
    y: i32,
    scale: PxScale,
    font: &FontRef,
    text: &str,
) {
    let scaled = font.as_scaled(scale);
    let mut cursor_x = x as f32;
    let baseline_y = y as f32 + scaled.ascent();
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    let data = pm.data_mut();

    let mut last_glyph_id = None;
    for c in text.chars() {
        let glyph_id = scaled.glyph_id(c);
        if let Some(last) = last_glyph_id {
            cursor_x += scaled.kern(last, glyph_id);
        }
        let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(cursor_x, baseline_y));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = gx as i32 + bounds.min.x as i32;
                let py = gy as i32 + bounds.min.y as i32;
                if px < 0 || px >= w || py < 0 || py >= h {
                    return;
                }
                let cov = coverage.clamp(0.0, 1.0);
                if cov < 0.01 {
                    return;
                }
                let idx = (py as usize * w as usize + px as usize) * 4;
                // Read existing premultiplied pixel
                let bg_r = data[idx] as f32;
                let bg_g = data[idx + 1] as f32;
                let bg_b = data[idx + 2] as f32;
                let bg_a = data[idx + 3] as f32;
                // Source color (premultiplied by coverage)
                let src_r = color[0] as f32 * cov;
                let src_g = color[1] as f32 * cov;
                let src_b = color[2] as f32 * cov;
                let src_a = 255.0 * cov;
                // Source-over compositing
                let inv_a = 1.0 - cov;
                data[idx] = (src_r + bg_r * inv_a).min(255.0) as u8;
                data[idx + 1] = (src_g + bg_g * inv_a).min(255.0) as u8;
                data[idx + 2] = (src_b + bg_b * inv_a).min(255.0) as u8;
                data[idx + 3] = (src_a + bg_a * inv_a).min(255.0) as u8;
            });
        }
        cursor_x += scaled.h_advance(glyph_id);
        last_glyph_id = Some(glyph_id);
    }
}

/// Measure the width and height of text at the given scale.
fn text_size(scale: PxScale, font: &FontRef, text: &str) -> (u32, u32) {
    let scaled = font.as_scaled(scale);
    let mut w = 0.0f32;
    let mut last_glyph_id = None;
    for c in text.chars() {
        let glyph_id = scaled.glyph_id(c);
        if let Some(last) = last_glyph_id {
            w += scaled.kern(last, glyph_id);
        }
        w += scaled.h_advance(glyph_id);
        last_glyph_id = Some(glyph_id);
    }
    let h = scaled.ascent() - scaled.descent();
    (w.ceil() as u32, h.ceil() as u32)
}

/// Draw text with a shadow (black offset by +1,+1).
fn draw_text_shadow(
    pm: &mut Pixmap,
    color: [u8; 3],
    x: i32,
    y: i32,
    scale: PxScale,
    font: &FontRef,
    text: &str,
) {
    draw_text(pm, [0, 0, 0], x + 1, y + 1, scale, font, text);
    draw_text(pm, color, x, y, scale, font, text);
}

// ── Drawing primitives ─────────────────────────────────────────────────────

/// Draw an anti-aliased line.
fn draw_line(
    pm: &mut Pixmap,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: [u8; 3],
    alpha: f32,
    width: f32,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    let Some(path) = pb.finish() else { return };
    let paint = color_paint(color, alpha);
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        miter_limit: 4.0,
        dash: None,
    };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draw an anti-aliased filled circle.
fn draw_filled_circle(pm: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: [u8; 3], alpha: f32) {
    let Some(path) = PathBuilder::from_circle(cx, cy, radius) else {
        return;
    };
    let paint = color_paint(color, alpha);
    pm.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Draw an anti-aliased circle outline.
fn draw_circle_outline(
    pm: &mut Pixmap,
    cx: f32,
    cy: f32,
    radius: f32,
    color: [u8; 3],
    alpha: f32,
    width: f32,
) {
    let Some(path) = PathBuilder::from_circle(cx, cy, radius) else {
        return;
    };
    let paint = color_paint(color, alpha);
    let stroke = Stroke {
        width,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Miter,
        miter_limit: 4.0,
        dash: None,
    };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draw an anti-aliased dashed circle outline.
fn draw_dashed_circle(
    pm: &mut Pixmap,
    cx: f32,
    cy: f32,
    radius: f32,
    color: [u8; 3],
    alpha: f32,
    width: f32,
) {
    let Some(path) = PathBuilder::from_circle(cx, cy, radius) else {
        return;
    };
    let paint = color_paint(color, alpha);
    // Dash pattern: 8px on, 8px off
    let dash = StrokeDash::new(vec![8.0, 8.0], 0.0);
    let stroke = Stroke {
        width,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Miter,
        miter_limit: 4.0,
        dash,
    };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draw a filled rectangle.
fn draw_filled_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: [u8; 3], alpha: f32) {
    let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let paint = color_paint(color, alpha);
    pm.fill_rect(rect, &paint, Transform::identity(), None);
}

// ── Composite drawing functions ────────────────────────────────────────────

/// Draw a capture point zone: filled circle + progress pie + outline + label.
fn draw_capture_point(
    pm: &mut Pixmap,
    x: f32,
    y: f32,
    radius: f32,
    color: [u8; 3],
    alpha: f32,
    label: &str,
    progress: f32,
    invader_color: Option<[u8; 3]>,
    font: &FontRef,
) {
    // Base filled circle with owner's color
    draw_filled_circle(pm, x, y, radius, color, alpha);

    // If capture in progress, draw a pie-slice fill in the invader's color
    if progress > 0.001 {
        if let Some(inv_color) = invader_color {
            let fill_alpha = alpha + 0.10;
            // Pie-slice from top (-PI/2), sweeping clockwise by progress * 2*PI
            let start_angle = -std::f32::consts::FRAC_PI_2;
            let sweep = progress * std::f32::consts::TAU;

            let mut pb = PathBuilder::new();
            pb.move_to(x, y);
            // Starting point on circle
            let sx = x + radius * (start_angle).cos();
            let sy = y + radius * (start_angle).sin();
            pb.line_to(sx, sy);

            // Approximate the arc with line segments (smooth enough at this scale)
            let steps = ((sweep / std::f32::consts::TAU) * 64.0).max(4.0) as i32;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let angle = start_angle + sweep * t;
                let px = x + radius * angle.cos();
                let py = y + radius * angle.sin();
                pb.line_to(px, py);
            }
            pb.close();

            if let Some(path) = pb.finish() {
                let paint = color_paint(inv_color, fill_alpha);
                pm.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    // Circle outline
    let outline_color = if invader_color.is_some() && progress > 0.001 {
        invader_color.unwrap()
    } else {
        color
    };
    draw_circle_outline(pm, x, y, radius, outline_color, 0.6, 2.0);

    // Centered label
    let scale = PxScale::from(16.0);
    let (tw, th) = text_size(scale, font, label);
    let tx = x as i32 - tw as i32 / 2;
    let ty = y as i32 - th as i32 / 2;
    draw_text_shadow(pm, [255, 255, 255], tx, ty, scale, font, label);
}

/// Draw player name and/or ship name labels centered above a ship icon.
fn draw_ship_labels(
    pm: &mut Pixmap,
    x: i32,
    y: i32,
    player_name: Option<&str>,
    ship_name: Option<&str>,
    name_color: Option<[u8; 3]>,
    font: &FontRef,
) {
    let scale = PxScale::from(10.0);
    let line_height = 12i32;
    let line_count = player_name.is_some() as i32 + ship_name.is_some() as i32;
    if line_count == 0 {
        return;
    }

    // Apply armament color to ship_name if shown, otherwise player_name
    let color_on_ship = ship_name.is_some();

    // Position lines above the icon (icon radius ~12px)
    let base_y = y - 14 - line_count * line_height;
    let mut cur_y = base_y;

    if let Some(name) = player_name {
        let color = if !color_on_ship {
            name_color.unwrap_or([255, 255, 255])
        } else {
            [255, 255, 255]
        };
        let (w, _) = text_size(scale, font, name);
        let tx = x - w as i32 / 2;
        draw_text_shadow(pm, color, tx, cur_y, scale, font, name);
        cur_y += line_height;
    }
    if let Some(name) = ship_name {
        let color = name_color.unwrap_or([255, 255, 255]);
        let (w, _) = text_size(scale, font, name);
        let tx = x - w as i32 / 2;
        draw_text_shadow(pm, color, tx, cur_y, scale, font, name);
    }
}

/// Draw a health bar below a ship icon.
fn draw_health_bar(
    pm: &mut Pixmap,
    x: i32,
    y: i32,
    fraction: f32,
    fill_color: [u8; 3],
    bg_color: [u8; 3],
    bg_alpha: f32,
) {
    let bar_w = 20.0f32;
    let bar_h = 3.0f32;
    let bar_x = x as f32 - bar_w / 2.0;
    let bar_y = y as f32 + 10.0;

    let fill_w = (fraction.clamp(0.0, 1.0) * bar_w).round();

    // Background portion
    if fill_w < bar_w {
        draw_filled_rect(
            pm,
            bar_x + fill_w,
            bar_y,
            bar_w - fill_w,
            bar_h,
            bg_color,
            bg_alpha,
        );
    }
    // Filled portion
    if fill_w > 0.0 {
        draw_filled_rect(pm, bar_x, bar_y, fill_w, bar_h, fill_color, 1.0);
    }
}

/// Draw a ship icon rotated by yaw, with optional team-color tinting.
///
/// Uses tiny-skia's bilinear-filtered transform compositing for smooth rotation.
fn draw_ship_icon(
    pm: &mut Pixmap,
    icon: &RgbaImage,
    x: i32,
    y: i32,
    yaw: f32,
    color: Option<[u8; 3]>,
    opacity: f32,
) {
    let iw = icon.width();
    let ih = icon.height();
    let cx = iw as f32 / 2.0;
    let cy = ih as f32 / 2.0;

    // Create a tinted copy of the icon as a Pixmap
    let mut icon_pm = Pixmap::new(iw, ih).expect("failed to create icon pixmap");
    let data = icon_pm.data_mut();
    for iy in 0..ih {
        for ix in 0..iw {
            let px = icon.get_pixel(ix, iy).0;
            let idx = (iy * iw + ix) as usize * 4;
            let a = px[3] as f32 / 255.0;
            if a < 0.01 {
                continue;
            }
            let (r, g, b) = if let Some(c) = color {
                // Tint: use luminance as intensity
                let luminance =
                    (px[0] as f32 * 0.299 + px[1] as f32 * 0.587 + px[2] as f32 * 0.114) / 255.0;
                (
                    (c[0] as f32 * luminance) as u8,
                    (c[1] as f32 * luminance) as u8,
                    (c[2] as f32 * luminance) as u8,
                )
            } else {
                (px[0], px[1], px[2])
            };
            // Premultiply
            data[idx] = (r as f32 * a) as u8;
            data[idx + 1] = (g as f32 * a) as u8;
            data[idx + 2] = (b as f32 * a) as u8;
            data[idx + 3] = px[3];
        }
    }

    // The SVG icons point upward (north = -Y). In game coordinates,
    // yaw=0 means east and increases counter-clockwise.
    // Screen rotation: R = PI/2 - yaw, converted to degrees for tiny-skia.
    let angle_deg = (std::f32::consts::FRAC_PI_2 - yaw).to_degrees();

    // Build transform: translate icon center to destination, then rotate
    let tx = x as f32 - cx;
    let ty = y as f32 - cy;
    let transform = Transform::from_translate(tx, ty).post_rotate_at(angle_deg, x as f32, y as f32);

    let paint = PixmapPaint {
        opacity,
        blend_mode: BlendMode::SourceOver,
        quality: FilterQuality::Bilinear,
    };

    pm.draw_pixmap(0, 0, icon_pm.as_ref(), &paint, transform, None);
}

/// Draw an outline around a ship icon's shape.
///
/// Draws the icon at slightly larger scale with outline color, then the normal icon on top.
fn draw_ship_icon_outline(
    pm: &mut Pixmap,
    icon: &RgbaImage,
    x: i32,
    y: i32,
    yaw: f32,
    outline_color: [u8; 3],
    outline_opacity: f32,
    thickness: i32,
) {
    // Draw outline by rendering the icon shifted in 8 directions
    let offsets: &[(i32, i32)] = &[
        (-thickness, 0),
        (thickness, 0),
        (0, -thickness),
        (0, thickness),
        (-thickness, -thickness),
        (thickness, -thickness),
        (-thickness, thickness),
        (thickness, thickness),
    ];
    for (dx, dy) in offsets {
        draw_ship_icon(
            pm,
            icon,
            x + dx,
            y + dy,
            yaw,
            Some(outline_color),
            outline_opacity,
        );
    }
}

/// Draw a plane/consumable icon (pre-colored RGBA, no rotation).
fn draw_icon(pm: &mut Pixmap, icon: &RgbaImage, x: i32, y: i32) {
    let iw = icon.width();
    let ih = icon.height();
    let icon_pm = rgba_to_pixmap(icon);
    let tx = x - iw as i32 / 2;
    let ty = y - ih as i32 / 2;
    let paint = PixmapPaint {
        opacity: 1.0,
        blend_mode: BlendMode::SourceOver,
        quality: FilterQuality::Bilinear,
    };
    pm.draw_pixmap(
        tx,
        ty,
        icon_pm.as_ref(),
        &paint,
        Transform::identity(),
        None,
    );
}

/// Draw the team score bar at the top of the frame.
///
/// Two independent progress bars growing toward the center. Each bar represents
/// progress toward 1000 points. Team 0 (friendly) grows left→center,
/// team 1 (enemy) grows right→center.
fn draw_score_bar(
    pm: &mut Pixmap,
    team0_score: i32,
    team1_score: i32,
    team0_color: [u8; 3],
    team1_color: [u8; 3],
    font: &FontRef,
) {
    let width = pm.width() as f32;
    let bar_height = 20.0f32;
    let max_score = 1000.0f32;
    let half = width / 2.0;
    let center_gap = 2.0f32; // small gap between the two bars

    // Dark background for the entire bar area
    draw_filled_rect(pm, 0.0, 0.0, width, bar_height, [30, 30, 30], 0.8);

    // Team 0 progress: grows from left edge toward center
    let t0_frac = (team0_score as f32 / max_score).clamp(0.0, 1.0);
    let t0_width = t0_frac * (half - center_gap);
    if t0_width > 0.0 {
        draw_filled_rect(pm, 0.0, 0.0, t0_width, bar_height, team0_color, 1.0);
    }

    // Team 1 progress: grows from right edge toward center
    let t1_frac = (team1_score as f32 / max_score).clamp(0.0, 1.0);
    let t1_width = t1_frac * (half - center_gap);
    if t1_width > 0.0 {
        draw_filled_rect(
            pm,
            width - t1_width,
            0.0,
            t1_width,
            bar_height,
            team1_color,
            1.0,
        );
    }

    // Score text — placed at outer edges to avoid overlapping the centered timer
    let scale = PxScale::from(14.0);
    let t0 = format!("{}", team0_score);
    let t1 = format!("{}", team1_score);
    let (t1w, _) = text_size(scale, font, &t1);
    // Team 0 score: near left edge
    draw_text_shadow(pm, [255, 255, 255], 8, 2, scale, font, &t0);
    // Team 1 score: near right edge
    draw_text_shadow(
        pm,
        [255, 255, 255],
        width as i32 - t1w as i32 - 8,
        2,
        scale,
        font,
        &t1,
    );
}

/// Draw the game timer.
fn draw_timer(pm: &mut Pixmap, seconds: f32, font: &FontRef) {
    let mins = (seconds as i32) / 60;
    let secs = (seconds as i32) % 60;
    let text = format!("{:02}:{:02}", mins, secs);
    let scale = PxScale::from(16.0);
    let (w, _) = text_size(scale, font, &text);
    let x = pm.width() as i32 / 2 - w as i32 / 2;
    draw_text_shadow(pm, [255, 255, 255], x, 2, scale, font, &text);
}

/// Map a DeathCause to the icon key used in the death_cause_icons HashMap.
///
/// Keys correspond to the base name portion of `icon_frag_{key}.png` files
/// in `gui/battle_hud/icon_frag/`.
fn death_cause_icon_key(cause: &wows_replays::analyzer::decoder::DeathCause) -> &'static str {
    use wows_replays::analyzer::decoder::DeathCause;
    match cause {
        DeathCause::Artillery | DeathCause::ApShell | DeathCause::HeShell | DeathCause::CsShell => {
            "main_caliber"
        }
        DeathCause::Secondaries => "atba",
        DeathCause::Torpedo | DeathCause::AerialTorpedo => "torpedo",
        DeathCause::Fire => "burning",
        DeathCause::Flooding => "flood",
        DeathCause::DiveBomber => "bomb",
        DeathCause::SkipBombs => "skip",
        DeathCause::AerialRocket => "rocket",
        DeathCause::Detonation => "detonate",
        DeathCause::Ramming => "ram",
        DeathCause::DepthCharge | DeathCause::AerialDepthCharge => "depthbomb",
        DeathCause::Missile => "missile",
        _ => "main_caliber",
    }
}

/// Draw rich kill feed entries in the top-right corner.
///
/// Layout per line (right-aligned):
/// `KILLER_NAME [icon] ship_name  [cause]  VICTIM_NAME [icon] ship_name`
fn draw_kill_feed(
    pm: &mut Pixmap,
    entries: &[KillFeedEntry],
    font: &FontRef,
    ship_icons: &HashMap<String, ShipIcon>,
    death_cause_icons: &HashMap<String, RgbaImage>,
) {
    let name_scale = PxScale::from(10.0);
    let ship_scale = PxScale::from(9.0);
    let line_height = 18i32;
    let right_margin = 4i32;
    let icon_size = (crate::assets::ICON_SIZE * 14 / 24) as i32;
    let cause_icon_size = icon_size;
    let gap = 2i32; // gap between elements
    let width = pm.width() as i32;

    for (i, entry) in entries.iter().take(5).enumerate() {
        let y = 22 + i as i32 * line_height;
        let icon_y = y - (line_height - icon_size) / 2;

        // Get death cause icon key
        let cause_key = death_cause_icon_key(&entry.cause);
        let has_cause_icon = death_cause_icons.contains_key(cause_key);
        let cause_w = if has_cause_icon {
            cause_icon_size
        } else {
            // Fallback to text measurement — shouldn't happen with full icon set
            0
        } as u32;

        // Measure all text segments
        let (killer_name_w, _) = text_size(name_scale, font, &entry.killer_name);
        let killer_ship = entry.killer_ship_name.as_deref().unwrap_or("");
        let (killer_ship_w, _) = if !killer_ship.is_empty() {
            text_size(ship_scale, font, killer_ship)
        } else {
            (0, 0)
        };
        let (victim_name_w, _) = text_size(name_scale, font, &entry.victim_name);
        let victim_ship = entry.victim_ship_name.as_deref().unwrap_or("");
        let (victim_ship_w, _) = if !victim_ship.is_empty() {
            text_size(ship_scale, font, victim_ship)
        } else {
            (0, 0)
        };

        // Determine if we have icons
        let has_killer_icon = entry.killer_species.is_some()
            && ship_icons.contains_key(entry.killer_species.as_ref().unwrap());
        let has_victim_icon = entry.victim_species.is_some()
            && ship_icons.contains_key(entry.victim_species.as_ref().unwrap());

        // Total width calculation:
        // killer_name [gap icon gap] killer_ship gap cause gap victim_name [gap icon gap] victim_ship
        let mut total_w = killer_name_w as i32;
        if has_killer_icon {
            total_w += gap + icon_size + gap;
        } else if killer_ship_w > 0 {
            total_w += gap;
        }
        if killer_ship_w > 0 {
            total_w += killer_ship_w as i32;
        }
        total_w += gap * 2 + cause_w as i32 + gap * 2;
        total_w += victim_name_w as i32;
        if has_victim_icon {
            total_w += gap + icon_size + gap;
        } else if victim_ship_w > 0 {
            total_w += gap;
        }
        if victim_ship_w > 0 {
            total_w += victim_ship_w as i32;
        }

        // Draw a semi-transparent background for readability
        let bg_x = (width - total_w - right_margin * 2) as f32;
        let bg_y = y as f32 - 1.0;
        draw_filled_rect(
            pm,
            bg_x,
            bg_y,
            (total_w + right_margin * 2) as f32,
            (line_height) as f32,
            [0, 0, 0],
            0.5,
        );

        let mut x = width - total_w - right_margin;

        // Killer name (team-colored)
        draw_text_shadow(
            pm,
            entry.killer_color,
            x,
            y,
            name_scale,
            font,
            &entry.killer_name,
        );
        x += killer_name_w as i32;

        // Killer ship icon (facing left = flipped horizontally)
        if has_killer_icon {
            x += gap;
            let icon = &ship_icons[entry.killer_species.as_ref().unwrap()];
            draw_kill_feed_icon(pm, icon, x, icon_y, icon_size, entry.killer_color, true);
            x += icon_size + gap;
        } else if killer_ship_w > 0 {
            x += gap;
        }

        // Killer ship name
        if killer_ship_w > 0 {
            draw_text_shadow(
                pm,
                entry.killer_color,
                x,
                y + 1,
                ship_scale,
                font,
                killer_ship,
            );
            x += killer_ship_w as i32;
        }

        // Death cause icon (or fallback gap)
        x += gap * 2;
        if let Some(cause_icon) = death_cause_icons.get(cause_key) {
            draw_icon(
                pm,
                cause_icon,
                x + cause_icon_size / 2,
                icon_y + cause_icon_size / 2,
            );
        }
        x += cause_w as i32 + gap * 2;

        // Victim name (team-colored)
        draw_text_shadow(
            pm,
            entry.victim_color,
            x,
            y,
            name_scale,
            font,
            &entry.victim_name,
        );
        x += victim_name_w as i32;

        // Victim ship icon (facing right = normal orientation)
        if has_victim_icon {
            x += gap;
            let icon = &ship_icons[entry.victim_species.as_ref().unwrap()];
            draw_kill_feed_icon(pm, icon, x, icon_y, icon_size, entry.victim_color, false);
            x += icon_size + gap;
        } else if victim_ship_w > 0 {
            x += gap;
        }

        // Victim ship name
        if victim_ship_w > 0 {
            draw_text_shadow(
                pm,
                entry.victim_color,
                x,
                y + 1,
                ship_scale,
                font,
                victim_ship,
            );
        }
    }
}

/// Draw a small ship icon for the kill feed, tinted with team color.
/// If `flip` is true, the icon faces left (horizontally mirrored).
fn draw_kill_feed_icon(
    pm: &mut Pixmap,
    icon: &RgbaImage,
    x: i32,
    y: i32,
    size: i32,
    color: [u8; 3],
    flip: bool,
) {
    let iw = icon.width();
    let ih = icon.height();
    let scale = size as f32 / iw.max(ih) as f32;

    // Create a tinted icon pixmap
    let mut icon_pm = Pixmap::new(iw, ih).expect("failed to create icon pixmap");
    let data = icon_pm.data_mut();
    for iy in 0..ih {
        for ix in 0..iw {
            let px = icon.get_pixel(ix, iy).0;
            let idx = (iy * iw + ix) as usize * 4;
            let a = px[3] as f32 / 255.0;
            if a < 0.01 {
                continue;
            }
            let luminance =
                (px[0] as f32 * 0.299 + px[1] as f32 * 0.587 + px[2] as f32 * 0.114) / 255.0;
            let r = (color[0] as f32 * luminance) as u8;
            let g = (color[1] as f32 * luminance) as u8;
            let b = (color[2] as f32 * luminance) as u8;
            // Premultiply
            data[idx] = (r as f32 * a) as u8;
            data[idx + 1] = (g as f32 * a) as u8;
            data[idx + 2] = (b as f32 * a) as u8;
            data[idx + 3] = px[3];
        }
    }

    // The ship icons point up (north). For kill feed we want them pointing
    // right (victim) or left (killer). Rotate 90° CW for right, 90° CCW for left.
    let angle_deg = if flip { -90.0 } else { 90.0 };

    let cx = iw as f32 / 2.0;
    let cy = ih as f32 / 2.0;
    // Center the icon at (x + size/2, y + size/2) with scaling
    let dest_cx = x as f32 + size as f32 / 2.0;
    let dest_cy = y as f32 + size as f32 / 2.0;

    let transform = Transform::from_translate(dest_cx - cx * scale, dest_cy - cy * scale)
        .pre_scale(scale, scale)
        .post_rotate_at(angle_deg, dest_cx, dest_cy);

    let paint = PixmapPaint {
        opacity: 1.0,
        blend_mode: BlendMode::SourceOver,
        quality: FilterQuality::Bilinear,
    };

    pm.draw_pixmap(0, 0, icon_pm.as_ref(), &paint, transform, None);
}

/// Draw the 10x10 grid overlay with labels.
fn draw_grid(pm: &mut Pixmap, minimap_size: u32, y_off: u32, font: &FontRef) {
    let cell = minimap_size as f32 / 10.0;
    let grid_color = [180, 180, 180];
    let alpha = 0.25f32;
    let label_scale = PxScale::from(11.0);

    // Draw 9 interior lines in each direction
    for i in 1..10 {
        let pos = (i as f32 * cell).round();
        // Vertical line
        draw_line(
            pm,
            pos,
            y_off as f32,
            pos,
            (y_off + minimap_size) as f32,
            grid_color,
            alpha,
            1.0,
        );
        // Horizontal line
        draw_line(
            pm,
            0.0,
            pos + y_off as f32,
            minimap_size as f32,
            pos + y_off as f32,
            grid_color,
            alpha,
            1.0,
        );
    }

    // Labels: numbers 1-10 across the top, letters A-J down the left
    for i in 0..10 {
        let label = format!("{}", i + 1);
        let x = (i as f32 * cell + cell / 2.0 - 3.0) as i32;
        let y = y_off as i32 + 2;
        draw_text_shadow(pm, [255, 255, 255], x, y, label_scale, font, &label);
    }
    let labels_row = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J'];
    for (i, &ch) in labels_row.iter().enumerate() {
        let label = ch.to_string();
        let x = 3i32;
        let y = y_off as i32 + (i as f32 * cell + cell / 2.0 - 5.0) as i32;
        draw_text_shadow(pm, [255, 255, 255], x, y, label_scale, font, &label);
    }
}

// ── ImageTarget (RenderTarget implementation) ──────────────────────────────

use crate::{CANVAS_HEIGHT, HUD_HEIGHT, MINIMAP_SIZE};

/// Pre-rasterized ship icon (RGBA, white/alpha mask to be tinted at draw time).
pub type ShipIcon = RgbaImage;

/// Software renderer that draws to a tiny-skia `Pixmap` for anti-aliased output.
///
/// Owns the map image, font, ship icons, plane icons, and consumable icons.
/// Implements `RenderTarget` by dispatching `DrawCommand`s to tiny-skia primitives.
pub struct ImageTarget {
    canvas: Pixmap,
    /// Pre-built background: map image + grid overlay. Cloned at start of each frame.
    base_canvas: Pixmap,
    font: FontRef<'static>,
    ship_icons: HashMap<String, ShipIcon>,
    plane_icons: HashMap<String, RgbaImage>,
    consumable_icons: HashMap<String, RgbaImage>,
    death_cause_icons: HashMap<String, RgbaImage>,
    powerup_icons: HashMap<String, RgbaImage>,
}

impl ImageTarget {
    pub fn new(
        map_image: Option<RgbImage>,
        ship_icons: HashMap<String, ShipIcon>,
        plane_icons: HashMap<String, RgbaImage>,
        consumable_icons: HashMap<String, RgbaImage>,
        death_cause_icons: HashMap<String, RgbaImage>,
        powerup_icons: HashMap<String, RgbaImage>,
    ) -> Self {
        let map = map_image
            .unwrap_or_else(|| RgbImage::from_pixel(MINIMAP_SIZE, MINIMAP_SIZE, Rgb([30, 40, 60])));
        let font = load_font();

        // Pre-build the base canvas: dark background + map + grid
        let mut base_rgb = RgbImage::from_pixel(MINIMAP_SIZE, CANVAS_HEIGHT, Rgb([20, 25, 35]));
        for y in 0..map.height().min(MINIMAP_SIZE) {
            for x in 0..map.width().min(MINIMAP_SIZE) {
                base_rgb.put_pixel(x, y + HUD_HEIGHT, *map.get_pixel(x, y));
            }
        }
        let mut base = rgb_to_pixmap(&base_rgb);
        draw_grid(&mut base, MINIMAP_SIZE, HUD_HEIGHT, &font);

        Self {
            canvas: Pixmap::new(MINIMAP_SIZE, CANVAS_HEIGHT).unwrap(),
            base_canvas: base,
            font,
            ship_icons,
            plane_icons,
            consumable_icons,
            death_cause_icons,
            powerup_icons,
        }
    }

    /// Access the current frame as an RGB image (converted from Pixmap).
    pub fn frame(&self) -> RgbImage {
        pixmap_to_rgb(&self.canvas)
    }

    /// Canvas dimensions.
    pub fn canvas_size(&self) -> (u32, u32) {
        (MINIMAP_SIZE, CANVAS_HEIGHT)
    }
}

impl RenderTarget for ImageTarget {
    fn begin_frame(&mut self) {
        self.canvas = self.base_canvas.clone();
    }

    fn draw(&mut self, cmd: &DrawCommand) {
        let y_off = HUD_HEIGHT as f32;
        match cmd {
            DrawCommand::ShotTracer { from, to, color } => {
                draw_line(
                    &mut self.canvas,
                    from.x as f32,
                    from.y as f32 + y_off,
                    to.x as f32,
                    to.y as f32 + y_off,
                    *color,
                    1.0,
                    1.5,
                );
            }
            DrawCommand::Torpedo { pos, color } => {
                draw_filled_circle(
                    &mut self.canvas,
                    pos.x as f32,
                    pos.y as f32 + y_off,
                    2.5,
                    *color,
                    1.0,
                );
            }
            DrawCommand::Smoke {
                pos,
                radius,
                color,
                alpha,
            } => {
                draw_filled_circle(
                    &mut self.canvas,
                    pos.x as f32,
                    pos.y as f32 + y_off,
                    *radius as f32,
                    *color,
                    *alpha,
                );
            }
            DrawCommand::BuffZone {
                pos,
                radius,
                color,
                alpha,
                marker_name,
            } => {
                let cx = pos.x as f32;
                let cy = pos.y as f32 + y_off;
                let r = *radius as f32;
                // Filled circle
                draw_filled_circle(&mut self.canvas, cx, cy, r, *color, *alpha);
                // Border ring
                draw_circle_outline(&mut self.canvas, cx, cy, r, *color, 0.6, 1.5);
                // Draw powerup icon centered on zone
                if let Some(name) = marker_name {
                    if let Some(icon) = self.powerup_icons.get(name.as_str()) {
                        draw_icon(&mut self.canvas, icon, cx as i32, cy as i32);
                    }
                }
            }
            DrawCommand::CapturePoint {
                pos,
                radius,
                color,
                alpha,
                label,
                progress,
                invader_color,
            } => {
                draw_capture_point(
                    &mut self.canvas,
                    pos.x as f32,
                    pos.y as f32 + y_off,
                    *radius as f32,
                    *color,
                    *alpha,
                    label,
                    *progress,
                    *invader_color,
                    &self.font,
                );
            }
            DrawCommand::TurretDirection {
                pos,
                yaw,
                color,
                length,
            } => {
                let x = pos.x as f32;
                let y = pos.y as f32 + y_off;
                let dx = *length as f32 * yaw.cos();
                let dy = -*length as f32 * yaw.sin();
                draw_line(&mut self.canvas, x, y, x + dx, y + dy, *color, 0.7, 1.0);
            }
            DrawCommand::Building { pos, color, .. } => {
                draw_filled_circle(
                    &mut self.canvas,
                    pos.x as f32,
                    pos.y as f32 + y_off,
                    2.5,
                    *color,
                    1.0,
                );
            }
            DrawCommand::Ship {
                pos,
                yaw,
                species,
                color,
                visibility,
                opacity,
                is_self,
                player_name,
                ship_name,
                is_detected_teammate,
                name_color,
            } => {
                let x = pos.x;
                let y = pos.y + y_off as i32;

                let Some(sp) = species.as_ref() else {
                    return;
                };
                let variant_key = match (*visibility, *is_self) {
                    (ShipVisibility::Visible, true) => format!("{}_self", sp),
                    (ShipVisibility::Visible, false) => sp.clone(),
                    (ShipVisibility::MinimapOnly, _) => format!("{}_last_visible", sp),
                    (ShipVisibility::Undetected, _) => format!("{}_invisible", sp),
                };
                let icon = self
                    .ship_icons
                    .get(&variant_key)
                    .or_else(|| self.ship_icons.get(sp))
                    .unwrap_or_else(|| panic!("missing ship icon for '{}'", variant_key));

                // Draw outline for detected teammates
                if *is_detected_teammate {
                    draw_ship_icon_outline(
                        &mut self.canvas,
                        icon,
                        x,
                        y,
                        *yaw,
                        [255, 215, 0],
                        0.9,
                        2,
                    );
                }

                draw_ship_icon(
                    &mut self.canvas,
                    icon,
                    x,
                    y,
                    *yaw,
                    color.map(|c| c),
                    *opacity,
                );
                draw_ship_labels(
                    &mut self.canvas,
                    x,
                    y,
                    player_name.as_deref(),
                    ship_name.as_deref(),
                    *name_color,
                    &self.font,
                );
            }
            DrawCommand::HealthBar {
                pos,
                fraction,
                fill_color,
                background_color,
                background_alpha,
            } => {
                draw_health_bar(
                    &mut self.canvas,
                    pos.x,
                    pos.y + y_off as i32,
                    *fraction,
                    *fill_color,
                    *background_color,
                    *background_alpha,
                );
            }
            DrawCommand::DeadShip {
                pos,
                yaw,
                species,
                color,
                is_self,
                ..
            } => {
                let x = pos.x;
                let y = pos.y + y_off as i32;

                let Some(sp) = species.as_ref() else {
                    return;
                };
                let variant_key = if *is_self {
                    format!("{}_dead_self", sp)
                } else {
                    format!("{}_dead", sp)
                };
                let icon = self
                    .ship_icons
                    .get(&variant_key)
                    .or_else(|| self.ship_icons.get(sp))
                    .unwrap_or_else(|| panic!("missing ship icon for '{}'", variant_key));

                draw_ship_icon(&mut self.canvas, icon, x, y, *yaw, color.map(|c| c), 1.0);
            }
            DrawCommand::Plane { pos, icon_key } => {
                let icon = self
                    .plane_icons
                    .get(icon_key)
                    .unwrap_or_else(|| panic!("missing plane icon for '{}'", icon_key));
                draw_icon(&mut self.canvas, icon, pos.x, pos.y + y_off as i32);
            }
            DrawCommand::ConsumableRadius {
                pos,
                radius_px,
                color,
                alpha,
            } => {
                let x = pos.x as f32;
                let y = pos.y as f32 + y_off;
                // Semi-transparent filled circle
                draw_filled_circle(&mut self.canvas, x, y, *radius_px as f32, *color, *alpha);
                // Outline for visibility
                draw_circle_outline(&mut self.canvas, x, y, *radius_px as f32, *color, 0.5, 2.0);
            }
            DrawCommand::ConsumableIcons {
                pos,
                icon_keys,
                has_hp_bar,
                ..
            } => {
                let x = pos.x;
                let y = pos.y + y_off as i32;
                let base_y = if *has_hp_bar { y + 28 } else { y + 26 };
                let icon_size = 28i32;
                let gap = 1i32;
                let count = icon_keys.len() as i32;
                let total_w = count * icon_size + (count - 1) * gap;
                let start_x = x - total_w / 2 + icon_size / 2;
                for (i, icon_key) in icon_keys.iter().enumerate() {
                    if let Some(icon) = self.consumable_icons.get(icon_key) {
                        let ix = start_x + i as i32 * (icon_size + gap);
                        draw_icon(&mut self.canvas, icon, ix, base_y);
                    }
                }
            }
            DrawCommand::ScoreBar {
                team0,
                team1,
                team0_color,
                team1_color,
            } => {
                draw_score_bar(
                    &mut self.canvas,
                    *team0,
                    *team1,
                    *team0_color,
                    *team1_color,
                    &self.font,
                );
            }
            DrawCommand::Timer { seconds } => {
                draw_timer(&mut self.canvas, *seconds, &self.font);
            }
            DrawCommand::TeamBuffs {
                friendly_buffs,
                enemy_buffs,
            } => {
                let icon_size = 16i32;
                let gap = 2i32;
                let buff_y = 22i32;
                let count_scale = PxScale::from(10.0);

                // Friendly buffs: left side, starting from x=4
                let mut x = 4i32;
                for (marker, count) in friendly_buffs {
                    if let Some(icon) = self.powerup_icons.get(marker.as_str()) {
                        let resized = image::imageops::resize(
                            icon,
                            icon_size as u32,
                            icon_size as u32,
                            image::imageops::FilterType::Nearest,
                        );
                        draw_icon(
                            &mut self.canvas,
                            &resized,
                            x + icon_size / 2,
                            buff_y + icon_size / 2,
                        );
                        if *count > 1 {
                            let label = format!("{}", count);
                            draw_text_shadow(
                                &mut self.canvas,
                                [255, 255, 255],
                                x + icon_size,
                                buff_y + 4,
                                count_scale,
                                &self.font,
                                &label,
                            );
                            let (tw, _) = text_size(count_scale, &self.font, &label);
                            x += icon_size + tw as i32 + gap;
                        } else {
                            x += icon_size + gap;
                        }
                    }
                }

                // Enemy buffs: right side, starting from right edge
                let width = self.canvas.width() as i32;
                let mut x = width - 4;
                for (marker, count) in enemy_buffs {
                    if let Some(icon) = self.powerup_icons.get(marker.as_str()) {
                        let resized = image::imageops::resize(
                            icon,
                            icon_size as u32,
                            icon_size as u32,
                            image::imageops::FilterType::Nearest,
                        );
                        if *count > 1 {
                            let label = format!("{}", count);
                            let (tw, _) = text_size(count_scale, &self.font, &label);
                            x -= tw as i32;
                            draw_text_shadow(
                                &mut self.canvas,
                                [255, 255, 255],
                                x,
                                buff_y + 4,
                                count_scale,
                                &self.font,
                                &label,
                            );
                            x -= icon_size;
                        } else {
                            x -= icon_size;
                        }
                        draw_icon(
                            &mut self.canvas,
                            &resized,
                            x + icon_size / 2,
                            buff_y + icon_size / 2,
                        );
                        x -= gap;
                    }
                }
            }
            DrawCommand::PositionTrail { points, .. } => {
                let y_off_i = y_off as i32;
                for (pos, color) in points {
                    draw_filled_circle(
                        &mut self.canvas,
                        pos.x as f32,
                        (pos.y + y_off_i) as f32,
                        1.0,
                        *color,
                        1.0,
                    );
                }
            }
            DrawCommand::ShipConfigCircle {
                pos,
                radius_px,
                color,
                alpha,
                dashed,
                label,
                is_self,
                ..
            } => {
                if !is_self {
                    return;
                }
                let x = pos.x as f32;
                let y = pos.y as f32 + y_off;
                let r = *radius_px;
                if *dashed {
                    draw_dashed_circle(&mut self.canvas, x, y, r, *color, *alpha, 1.0);
                } else {
                    draw_circle_outline(&mut self.canvas, x, y, r, *color, *alpha, 1.0);
                }
                if let Some(text) = label {
                    let scale = PxScale::from(11.0);
                    let lx = x as i32 + r as i32 + 3;
                    let ly = y as i32 - 5;
                    draw_text_shadow(&mut self.canvas, *color, lx, ly, scale, &self.font, text);
                }
            }
            DrawCommand::KillFeed { entries } => {
                draw_kill_feed(
                    &mut self.canvas,
                    entries,
                    &self.font,
                    &self.ship_icons,
                    &self.death_cause_icons,
                );
            }
        }
    }

    fn end_frame(&mut self) {
        // No-op — frame is ready to read via frame()
    }
}
