use std::collections::HashMap;

use ab_glyph::{FontRef, PxScale};
use image::{Rgb, RgbImage, RgbaImage};
use imageproc::drawing::{
    draw_filled_circle_mut, draw_filled_rect_mut, draw_line_segment_mut, draw_text_mut, text_size,
};
use imageproc::rect::Rect;

use crate::draw_command::{DrawCommand, RenderTarget, ShipVisibility};

const COLOR_TEXT: Rgb<u8> = Rgb([255, 255, 255]);
const COLOR_TEXT_SHADOW: Rgb<u8> = Rgb([0, 0, 0]);

const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

fn load_font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_DATA).expect("failed to load embedded font")
}

/// Draw an artillery shot trajectory line.
fn draw_shot_line(image: &mut RgbImage, x1: f32, y1: f32, x2: f32, y2: f32, color: Rgb<u8>) {
    draw_line_segment_mut(image, (x1, y1), (x2, y2), color);
}

/// Draw a torpedo dot.
fn draw_torpedo(image: &mut RgbImage, x: i32, y: i32, color: Rgb<u8>) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < 0 || x >= w || y < 0 || y >= h {
        return;
    }
    draw_filled_circle_mut(image, (x, y), 2, color);
}

/// Draw a turret direction line with alpha blending.
fn draw_turret_line(image: &mut RgbImage, x1: i32, y1: i32, x2: i32, y2: i32, color: [u8; 3]) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    let alpha = 0.7f32;

    // Bresenham's line algorithm
    let dx = (x2 - x1).abs();
    let dy = -(y2 - y1).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut cx = x1;
    let mut cy = y1;

    loop {
        if cx >= 0 && cx < w && cy >= 0 && cy < h {
            let bg = image.get_pixel(cx as u32, cy as u32).0;
            let blended = Rgb([
                (color[0] as f32 * alpha + bg[0] as f32 * (1.0 - alpha)) as u8,
                (color[1] as f32 * alpha + bg[1] as f32 * (1.0 - alpha)) as u8,
                (color[2] as f32 * alpha + bg[2] as f32 * (1.0 - alpha)) as u8,
            ]);
            image.put_pixel(cx as u32, cy as u32, blended);
        }
        if cx == x2 && cy == y2 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
    }
}

/// Draw a circle outline with alpha blending.
///
/// Used for detected teammate highlight and consumable radius indicators.
fn draw_circle_outline(
    image: &mut RgbImage,
    x: i32,
    y: i32,
    radius: i32,
    color: [u8; 3],
    alpha: f32,
    thickness: i32,
) {
    let w = image.width() as i32;
    let h = image.height() as i32;

    // Draw multiple radius offsets for thickness
    for r_off in 0..thickness {
        let r = radius - r_off;
        if r <= 0 {
            continue;
        }
        // Use angle stepping for smooth circle
        for angle_step in 0..720 {
            let angle = (angle_step as f32 * 0.5).to_radians();
            let px = x + (r as f32 * angle.cos()).round() as i32;
            let py = y + (r as f32 * angle.sin()).round() as i32;
            if px >= 0 && px < w && py >= 0 && py < h {
                let bg = image.get_pixel(px as u32, py as u32).0;
                let blended = Rgb([
                    (color[0] as f32 * alpha + bg[0] as f32 * (1.0 - alpha)) as u8,
                    (color[1] as f32 * alpha + bg[1] as f32 * (1.0 - alpha)) as u8,
                    (color[2] as f32 * alpha + bg[2] as f32 * (1.0 - alpha)) as u8,
                ]);
                image.put_pixel(px as u32, py as u32, blended);
            }
        }
    }
}

/// Draw a dashed circle outline with alpha blending.
fn draw_dashed_circle_outline(
    image: &mut RgbImage,
    x: i32,
    y: i32,
    radius: i32,
    color: [u8; 3],
    alpha: f32,
    thickness: i32,
) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    // Dash pattern: 8px on, 8px off (in angle steps)
    const DASH_ON: i32 = 16;
    const DASH_CYCLE: i32 = 32;

    for r_off in 0..thickness {
        let r = radius - r_off;
        if r <= 0 {
            continue;
        }
        for angle_step in 0..720 {
            if angle_step % DASH_CYCLE >= DASH_ON {
                continue;
            }
            let angle = (angle_step as f32 * 0.5).to_radians();
            let px = x + (r as f32 * angle.cos()).round() as i32;
            let py = y + (r as f32 * angle.sin()).round() as i32;
            if px >= 0 && px < w && py >= 0 && py < h {
                let bg = image.get_pixel(px as u32, py as u32).0;
                let blended = Rgb([
                    (color[0] as f32 * alpha + bg[0] as f32 * (1.0 - alpha)) as u8,
                    (color[1] as f32 * alpha + bg[1] as f32 * (1.0 - alpha)) as u8,
                    (color[2] as f32 * alpha + bg[2] as f32 * (1.0 - alpha)) as u8,
                ]);
                image.put_pixel(px as u32, py as u32, blended);
            }
        }
    }
}

/// Draw a smoke screen as a semi-transparent filled circle.
fn draw_smoke(image: &mut RgbImage, x: i32, y: i32, radius: i32, smoke_color: [u8; 3], alpha: f32) {
    let w = image.width() as i32;
    let h = image.height() as i32;

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let px = x + dx;
            let py = y + dy;
            if px < 0 || px >= w || py < 0 || py >= h {
                continue;
            }
            let bg = image.get_pixel(px as u32, py as u32).0;
            let blended = Rgb([
                (bg[0] as f32 * (1.0 - alpha) + smoke_color[0] as f32 * alpha) as u8,
                (bg[1] as f32 * (1.0 - alpha) + smoke_color[1] as f32 * alpha) as u8,
                (bg[2] as f32 * (1.0 - alpha) + smoke_color[2] as f32 * alpha) as u8,
            ]);
            image.put_pixel(px as u32, py as u32, blended);
        }
    }
}

/// Draw a plane icon (pre-colored RGBA from game files) with alpha blending.
fn draw_plane_icon(image: &mut RgbImage, icon: &RgbaImage, x: i32, y: i32) {
    let iw = icon.width() as i32;
    let ih = icon.height() as i32;
    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    for dy in 0..ih {
        for dx in 0..iw {
            let dest_x = x - iw / 2 + dx;
            let dest_y = y - ih / 2 + dy;
            if dest_x < 0 || dest_x >= img_w || dest_y < 0 || dest_y >= img_h {
                continue;
            }
            let pixel = icon.get_pixel(dx as u32, dy as u32);
            let alpha = pixel[3] as f32 / 255.0;
            if alpha < 0.05 {
                continue;
            }
            let bg = image.get_pixel(dest_x as u32, dest_y as u32).0;
            let blended = Rgb([
                (bg[0] as f32 * (1.0 - alpha) + pixel[0] as f32 * alpha) as u8,
                (bg[1] as f32 * (1.0 - alpha) + pixel[1] as f32 * alpha) as u8,
                (bg[2] as f32 * (1.0 - alpha) + pixel[2] as f32 * alpha) as u8,
            ]);
            image.put_pixel(dest_x as u32, dest_y as u32, blended);
        }
    }
}

/// Draw a capture point zone: filled circle + outline + centered label.
fn draw_capture_point(
    image: &mut RgbImage,
    x: i32,
    y: i32,
    radius: i32,
    color: [u8; 3],
    alpha: f32,
    label: &str,
    progress: f32,
    invader_color: Option<[u8; 3]>,
    font: &FontRef,
) {
    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    // Base filled circle with owner's color
    draw_smoke(image, x, y, radius, color, alpha);

    // If capture in progress, draw a pie-slice fill in the invader's color
    if progress > 0.001 {
        if let Some(inv_color) = invader_color {
            let fill_alpha = alpha + 0.10;
            // Pie-slice from top (-PI/2), sweeping clockwise by progress * 2*PI
            let start_angle = -std::f32::consts::FRAC_PI_2;
            let sweep = progress * std::f32::consts::TAU;
            let r2 = (radius * radius) as f32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let dist2 = (dx * dx + dy * dy) as f32;
                    if dist2 > r2 {
                        continue;
                    }
                    // Check if this pixel is within the pie-slice
                    let mut angle = (dy as f32).atan2(dx as f32) - start_angle;
                    if angle < 0.0 {
                        angle += std::f32::consts::TAU;
                    }
                    if angle > sweep {
                        continue;
                    }
                    let px = x + dx;
                    let py = y + dy;
                    if px >= 0 && px < img_w && py >= 0 && py < img_h {
                        let bg = image.get_pixel(px as u32, py as u32).0;
                        let blended = Rgb([
                            (inv_color[0] as f32 * fill_alpha + bg[0] as f32 * (1.0 - fill_alpha))
                                as u8,
                            (inv_color[1] as f32 * fill_alpha + bg[1] as f32 * (1.0 - fill_alpha))
                                as u8,
                            (inv_color[2] as f32 * fill_alpha + bg[2] as f32 * (1.0 - fill_alpha))
                                as u8,
                        ]);
                        image.put_pixel(px as u32, py as u32, blended);
                    }
                }
            }
        }
    }

    // Circle outline — use invader color when contested, owner color otherwise
    let outline_color = if invader_color.is_some() && progress > 0.001 {
        invader_color.unwrap()
    } else {
        color
    };
    let outline_alpha = 0.6f32;
    for angle_step in 0..720 {
        let angle = (angle_step as f32 * 0.5).to_radians();
        for r_offset in [radius, radius - 1] {
            let px = x + (r_offset as f32 * angle.cos()).round() as i32;
            let py = y + (r_offset as f32 * angle.sin()).round() as i32;
            if px >= 0 && px < img_w && py >= 0 && py < img_h {
                let bg = image.get_pixel(px as u32, py as u32).0;
                let blended = Rgb([
                    (outline_color[0] as f32 * outline_alpha + bg[0] as f32 * (1.0 - outline_alpha))
                        as u8,
                    (outline_color[1] as f32 * outline_alpha + bg[1] as f32 * (1.0 - outline_alpha))
                        as u8,
                    (outline_color[2] as f32 * outline_alpha + bg[2] as f32 * (1.0 - outline_alpha))
                        as u8,
                ]);
                image.put_pixel(px as u32, py as u32, blended);
            }
        }
    }

    // Centered label
    let scale = PxScale::from(16.0);
    let (w, h) = text_size(scale, font, label);
    let tx = x - w as i32 / 2;
    let ty = y - h as i32 / 2;
    draw_text_mut(image, COLOR_TEXT_SHADOW, tx + 1, ty + 1, scale, font, label);
    draw_text_mut(image, COLOR_TEXT, tx, ty, scale, font, label);
}

/// Draw player name and/or ship name labels centered above a ship icon.
///
/// When both are present, player name is on top, ship name below it.
/// When only one is present, it occupies the single line closest to the icon.
fn draw_ship_labels(
    image: &mut RgbImage,
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
            name_color.map(Rgb).unwrap_or(COLOR_TEXT)
        } else {
            COLOR_TEXT
        };
        let (w, _) = text_size(scale, font, name);
        let tx = x - w as i32 / 2;
        draw_text_mut(
            image,
            COLOR_TEXT_SHADOW,
            tx + 1,
            cur_y + 1,
            scale,
            font,
            name,
        );
        draw_text_mut(image, color, tx, cur_y, scale, font, name);
        cur_y += line_height;
    }
    if let Some(name) = ship_name {
        let color = name_color.map(Rgb).unwrap_or(COLOR_TEXT);
        let (w, _) = text_size(scale, font, name);
        let tx = x - w as i32 / 2;
        draw_text_mut(
            image,
            COLOR_TEXT_SHADOW,
            tx + 1,
            cur_y + 1,
            scale,
            font,
            name,
        );
        draw_text_mut(image, color, tx, cur_y, scale, font, name);
    }
}

use crate::{CANVAS_HEIGHT, HUD_HEIGHT, MINIMAP_SIZE};

/// Pre-rasterized ship icon (RGBA, white/alpha mask to be tinted at draw time).
pub type ShipIcon = RgbaImage;

/// Software renderer that draws to an `RgbImage`.
///
/// Owns the map image, font, ship icons, plane icons, and consumable icons.
/// Implements `RenderTarget` by dispatching `DrawCommand`s to pixel-level helpers.
pub struct ImageTarget {
    canvas: RgbImage,
    /// Pre-built background: map image + grid overlay. Cloned at start of each frame.
    base_canvas: RgbImage,
    font: FontRef<'static>,
    ship_icons: HashMap<String, ShipIcon>,
    plane_icons: HashMap<String, RgbaImage>,
    consumable_icons: HashMap<String, RgbaImage>,
}

impl ImageTarget {
    pub fn new(
        map_image: Option<RgbImage>,
        ship_icons: HashMap<String, ShipIcon>,
        plane_icons: HashMap<String, RgbaImage>,
        consumable_icons: HashMap<String, RgbaImage>,
    ) -> Self {
        let map = map_image
            .unwrap_or_else(|| RgbImage::from_pixel(MINIMAP_SIZE, MINIMAP_SIZE, Rgb([30, 40, 60])));
        let font = load_font();

        // Pre-build the base canvas: dark background + map + grid
        let mut base = RgbImage::from_pixel(MINIMAP_SIZE, CANVAS_HEIGHT, Rgb([20, 25, 35]));
        for y in 0..map.height().min(MINIMAP_SIZE) {
            for x in 0..map.width().min(MINIMAP_SIZE) {
                base.put_pixel(x, y + HUD_HEIGHT, *map.get_pixel(x, y));
            }
        }
        draw_grid(&mut base, MINIMAP_SIZE, HUD_HEIGHT, &font);

        Self {
            canvas: RgbImage::new(MINIMAP_SIZE, CANVAS_HEIGHT),
            base_canvas: base,
            font,
            ship_icons,
            plane_icons,
            consumable_icons,
        }
    }

    /// Access the current frame image.
    pub fn frame(&self) -> &RgbImage {
        &self.canvas
    }

    /// Canvas dimensions.
    pub fn canvas_size(&self) -> (u32, u32) {
        (MINIMAP_SIZE, CANVAS_HEIGHT)
    }
}

impl RenderTarget for ImageTarget {
    fn begin_frame(&mut self) {
        // Clone the pre-built base canvas (map + grid)
        self.canvas = self.base_canvas.clone();
    }

    fn draw(&mut self, cmd: &DrawCommand) {
        let y_off = HUD_HEIGHT as i32;
        match cmd {
            DrawCommand::ShotTracer { from, to, color } => {
                draw_shot_line(
                    &mut self.canvas,
                    from.x as f32,
                    from.y as f32 + y_off as f32,
                    to.x as f32,
                    to.y as f32 + y_off as f32,
                    Rgb(*color),
                );
            }
            DrawCommand::Torpedo { pos, color } => {
                draw_torpedo(&mut self.canvas, pos.x, pos.y + y_off, Rgb(*color));
            }
            DrawCommand::Smoke {
                pos,
                radius,
                color,
                alpha,
            } => {
                draw_smoke(
                    &mut self.canvas,
                    pos.x,
                    pos.y + y_off,
                    *radius,
                    *color,
                    *alpha,
                );
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
                let x = pos.x;
                let y = pos.y + y_off;
                draw_capture_point(
                    &mut self.canvas,
                    x,
                    y,
                    *radius,
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
                let x = pos.x;
                let y = pos.y + y_off;
                // Draw a line from ship center in the turret yaw direction
                // Game yaw: 0 = east, PI/2 = north; screen: 0 = right, PI/2 = up
                let dx = (*length as f32 * yaw.cos()).round() as i32;
                let dy = (-*length as f32 * yaw.sin()).round() as i32;
                let x2 = x + dx;
                let y2 = y + dy;
                draw_turret_line(&mut self.canvas, x, y, x2, y2, *color);
            }
            DrawCommand::Building {
                pos,
                color,
                is_alive,
            } => {
                let x = pos.x;
                let y = pos.y + y_off;
                if *is_alive {
                    draw_filled_circle_mut(&mut self.canvas, (x, y), 2, Rgb(*color));
                } else {
                    draw_filled_circle_mut(&mut self.canvas, (x, y), 2, Rgb(*color));
                }
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
                let rgb = color.map(Rgb);
                let x = pos.x;
                let y = pos.y + y_off;

                // Pick the right icon variant based on visibility and self status
                let Some(sp) = species.as_ref() else {
                    // No species info — can't render an icon, skip
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

                // Draw icon-shape outline for detected teammates (before icon)
                if *is_detected_teammate {
                    const GOLD: Rgb<u8> = Rgb([255, 215, 0]);
                    draw_ship_icon_outline(&mut self.canvas, icon, x, y, *yaw, GOLD, 0.9, 2);
                }

                draw_ship_icon(&mut self.canvas, icon, x, y, *yaw, rgb, *opacity);
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
                    pos.y + y_off,
                    *fraction,
                    Rgb(*fill_color),
                    Rgb(*background_color),
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
                let y = pos.y + y_off;
                let rgb = color.map(Rgb);

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

                draw_ship_icon(&mut self.canvas, icon, x, y, *yaw, rgb, 1.0);
                // Skip labels for dead ships in video output — just show the X marker
            }
            DrawCommand::Plane { pos, icon_key } => {
                let icon = self
                    .plane_icons
                    .get(icon_key)
                    .unwrap_or_else(|| panic!("missing plane icon for '{}'", icon_key));
                draw_plane_icon(&mut self.canvas, icon, pos.x, pos.y + y_off);
            }
            DrawCommand::ConsumableRadius {
                pos,
                radius_px,
                color,
                alpha,
            } => {
                let x = pos.x;
                let y = pos.y + y_off;
                // Draw semi-transparent filled circle
                draw_smoke(&mut self.canvas, x, y, *radius_px, *color, *alpha);
                // Draw outline for better visibility
                draw_circle_outline(&mut self.canvas, x, y, *radius_px, *color, 0.5, 2);
            }
            DrawCommand::ConsumableIcons {
                pos,
                icon_keys,
                has_hp_bar,
                ..
            } => {
                let x = pos.x;
                let y = pos.y + y_off;
                // Position below health bar (y+10 bar top + 3 bar height + 2 gap = y+15)
                // or below the ship icon if no HP bar (y + 12 + 2 gap = y+14)
                let base_y = if *has_hp_bar { y + 28 } else { y + 26 };
                let icon_size = 28i32;
                let gap = 1i32;
                let count = icon_keys.len() as i32;
                let total_w = count * icon_size + (count - 1) * gap;
                let start_x = x - total_w / 2 + icon_size / 2;
                for (i, icon_key) in icon_keys.iter().enumerate() {
                    if let Some(icon) = self.consumable_icons.get(icon_key) {
                        let ix = start_x + i as i32 * (icon_size + gap);
                        draw_plane_icon(&mut self.canvas, icon, ix, base_y);
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
                    Rgb(*team0_color),
                    Rgb(*team1_color),
                    &self.font,
                );
            }
            DrawCommand::Timer { seconds } => {
                draw_timer(&mut self.canvas, *seconds, &self.font);
            }
            DrawCommand::PositionTrail { points, .. } => {
                for (pos, color) in points {
                    let px = pos.x;
                    let py = pos.y + y_off;
                    if px >= 0
                        && (px as u32) < self.canvas.width()
                        && py >= 0
                        && (py as u32) < self.canvas.height()
                    {
                        self.canvas.put_pixel(px as u32, py as u32, Rgb(*color));
                    }
                }
            }
            DrawCommand::ShipConfigCircle {
                pos,
                radius_px,
                color,
                alpha,
                dashed,
                label,
                ..
            } => {
                let x = pos.x;
                let y = pos.y + y_off;
                let r = *radius_px as i32;
                if *dashed {
                    draw_dashed_circle_outline(&mut self.canvas, x, y, r, *color, *alpha, 1);
                } else {
                    draw_circle_outline(&mut self.canvas, x, y, r, *color, *alpha, 1);
                }
                if let Some(text) = label {
                    let scale = ab_glyph::PxScale::from(11.0);
                    let lx = x + r + 3;
                    let ly = y - 5;
                    draw_text_mut(
                        &mut self.canvas,
                        COLOR_TEXT_SHADOW,
                        lx + 1,
                        ly + 1,
                        scale,
                        &self.font,
                        text,
                    );
                    draw_text_mut(
                        &mut self.canvas,
                        Rgb(*color),
                        lx,
                        ly,
                        scale,
                        &self.font,
                        text,
                    );
                }
            }
            DrawCommand::KillFeed { entries } => {
                draw_kill_feed(&mut self.canvas, entries, &self.font);
            }
        }
    }

    fn end_frame(&mut self) {
        // No-op — frame is ready to read via frame()
    }
}

/// Draw the team score bar at the top of the frame.
fn draw_score_bar(
    image: &mut RgbImage,
    team0_score: i32,
    team1_score: i32,
    team0_color: Rgb<u8>,
    team1_color: Rgb<u8>,
    font: &FontRef,
) {
    let width = image.width();
    let bar_height = 20u32;
    let total = (team0_score + team1_score).max(1) as f32;
    let team0_width = ((team0_score as f32 / total) * width as f32) as u32;

    // Team 0 bar
    if team0_width > 0 {
        draw_filled_rect_mut(
            image,
            Rect::at(0, 0).of_size(team0_width, bar_height),
            team0_color,
        );
    }
    // Team 1 bar
    if team0_width < width {
        draw_filled_rect_mut(
            image,
            Rect::at(team0_width as i32, 0).of_size(width - team0_width, bar_height),
            team1_color,
        );
    }

    // Score text
    let scale = PxScale::from(14.0);
    let team0_text = format!("{}", team0_score);
    let team1_text = format!("{}", team1_score);
    draw_text_mut(image, COLOR_TEXT, 5, 3, scale, font, &team0_text);
    let team1_x = width as i32 - (team1_text.len() as i32 * 9) - 5;
    draw_text_mut(image, COLOR_TEXT, team1_x, 3, scale, font, &team1_text);
}

/// Draw the game timer.
fn draw_timer(image: &mut RgbImage, game_time_secs: f32, font: &FontRef) {
    let total_secs = game_time_secs.max(0.0) as u32;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    let text = format!("{:02}:{:02}", minutes, seconds);

    let scale = PxScale::from(16.0);
    let width = image.width();
    let x = (width / 2) as i32 - 20;

    // Shadow
    draw_text_mut(image, COLOR_TEXT_SHADOW, x + 1, 4, scale, font, &text);
    // Text
    draw_text_mut(image, COLOR_TEXT, x, 3, scale, font, &text);
}

/// Draw the kill feed in the top-right corner.
fn draw_kill_feed(
    image: &mut RgbImage,
    kills: &[(String, String)], // (killer_name, victim_name)
    font: &FontRef,
) {
    let scale = PxScale::from(11.0);
    let width = image.width() as i32;
    let mut y = 25;

    for (killer, victim) in kills.iter().take(5) {
        let text = format!("{} > {}", killer, victim);
        let x = width - (text.len() as i32 * 7) - 5;
        // Shadow
        draw_text_mut(image, COLOR_TEXT_SHADOW, x + 1, y + 1, scale, font, &text);
        // Text
        draw_text_mut(image, COLOR_TEXT, x, y, scale, font, &text);
        y += 14;
    }
}

/// Draw A-J / 1-10 grid lines and labels over the minimap area.
///
/// `y_off` is the vertical offset from the top of the canvas to the start of the map.
fn draw_grid(image: &mut RgbImage, minimap_size: u32, y_off: u32, font: &FontRef) {
    let grid_color = Rgb([255u8, 255, 255]);
    let alpha = 0.25f32;
    let cell = minimap_size as f32 / 10.0;
    let label_scale = PxScale::from(11.0);

    // Draw 9 vertical and 9 horizontal interior lines (blended for transparency)
    for i in 1..10 {
        let pos = (i as f32 * cell).round() as i32;

        // Vertical line
        for y in 0..minimap_size as i32 {
            let px = pos;
            let py = y + y_off as i32;
            if px >= 0 && (px as u32) < minimap_size {
                let bg = image.get_pixel(px as u32, py as u32).0;
                let blended = Rgb([
                    (grid_color[0] as f32 * alpha + bg[0] as f32 * (1.0 - alpha)) as u8,
                    (grid_color[1] as f32 * alpha + bg[1] as f32 * (1.0 - alpha)) as u8,
                    (grid_color[2] as f32 * alpha + bg[2] as f32 * (1.0 - alpha)) as u8,
                ]);
                image.put_pixel(px as u32, py as u32, blended);
            }
        }

        // Horizontal line
        for x in 0..minimap_size as i32 {
            let px = x;
            let py = pos + y_off as i32;
            if py >= 0 && (py as u32) < image.height() {
                let bg = image.get_pixel(px as u32, py as u32).0;
                let blended = Rgb([
                    (grid_color[0] as f32 * alpha + bg[0] as f32 * (1.0 - alpha)) as u8,
                    (grid_color[1] as f32 * alpha + bg[1] as f32 * (1.0 - alpha)) as u8,
                    (grid_color[2] as f32 * alpha + bg[2] as f32 * (1.0 - alpha)) as u8,
                ]);
                image.put_pixel(px as u32, py as u32, blended);
            }
        }
    }

    // Labels: numbers 1-10 across the top, letters A-J down the left
    for i in 0..10 {
        let label = format!("{}", i + 1);
        let x = (i as f32 * cell + cell / 2.0 - 3.0) as i32;
        let y = y_off as i32 + 2;
        draw_text_mut(
            image,
            COLOR_TEXT_SHADOW,
            x + 1,
            y + 1,
            label_scale,
            font,
            &label,
        );
        draw_text_mut(image, COLOR_TEXT, x, y, label_scale, font, &label);
    }
    let labels_row = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J'];
    for (i, &ch) in labels_row.iter().enumerate() {
        let label = ch.to_string();
        let x = 3i32;
        let y = y_off as i32 + (i as f32 * cell + cell / 2.0 - 5.0) as i32;
        draw_text_mut(
            image,
            COLOR_TEXT_SHADOW,
            x + 1,
            y + 1,
            label_scale,
            font,
            &label,
        );
        draw_text_mut(image, COLOR_TEXT, x, y, label_scale, font, &label);
    }
}

/// Draw a health bar below a ship icon.
///
/// `fraction` is current health / max health (0.0 to 1.0).
fn draw_health_bar(
    image: &mut RgbImage,
    x: i32,
    y: i32,
    fraction: f32,
    fill_color: Rgb<u8>,
    bg_color: Rgb<u8>,
    bg_alpha: f32,
) {
    let bar_w = 20i32;
    let bar_h = 3i32;
    let bar_x = x - bar_w / 2;
    let bar_y = y + 10; // below the ship icon

    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    let fill_w = (fraction.clamp(0.0, 1.0) * bar_w as f32).round() as i32;

    for dy in 0..bar_h {
        for dx in 0..bar_w {
            let px = bar_x + dx;
            let py = bar_y + dy;
            if px < 0 || px >= img_w || py < 0 || py >= img_h {
                continue;
            }
            let bg = image.get_pixel(px as u32, py as u32).0;
            if dx < fill_w {
                // Filled portion
                image.put_pixel(px as u32, py as u32, fill_color);
            } else {
                // Empty portion (semi-transparent dark background)
                let blended = Rgb([
                    (bg_color[0] as f32 * bg_alpha + bg[0] as f32 * (1.0 - bg_alpha)) as u8,
                    (bg_color[1] as f32 * bg_alpha + bg[1] as f32 * (1.0 - bg_alpha)) as u8,
                    (bg_color[2] as f32 * bg_alpha + bg[2] as f32 * (1.0 - bg_alpha)) as u8,
                ]);
                image.put_pixel(px as u32, py as u32, blended);
            }
        }
    }
}

/// Draw an outline that follows the ship icon's shape.
///
/// For each destination pixel, we check whether any icon pixel within `thickness` distance
/// has non-zero alpha (via the rotated inverse-sample). If a neighbor is opaque but the
/// center pixel itself is transparent, we draw the outline color there.
fn draw_ship_icon_outline(
    image: &mut RgbImage,
    icon: &RgbaImage,
    x: i32,
    y: i32,
    yaw: f32,
    color: Rgb<u8>,
    opacity: f32,
    thickness: i32,
) {
    let iw = icon.width() as i32;
    let ih = icon.height() as i32;
    let cx = iw as f32 / 2.0;
    let cy = ih as f32 / 2.0;
    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    let cos_r = yaw.sin();
    let sin_r = yaw.cos();

    let half = iw / 2 + thickness;

    for dy in -half..=half {
        for dx in -half..=half {
            let dest_x = x + dx;
            let dest_y = y + dy;
            if dest_x < 0 || dest_x >= img_w || dest_y < 0 || dest_y >= img_h {
                continue;
            }

            // Check if this pixel itself is inside the icon (we only want the border)
            let fdx = dx as f32;
            let fdy = dy as f32;
            let src_x = fdx * cos_r + fdy * sin_r + cx;
            let src_y = -fdx * sin_r + fdy * cos_r + cy;
            let sx = src_x.round() as i32;
            let sy = src_y.round() as i32;
            let self_opaque = if sx >= 0 && sx < iw && sy >= 0 && sy < ih {
                icon.get_pixel(sx as u32, sy as u32)[3] > 128
            } else {
                false
            };
            if self_opaque {
                // This pixel will be covered by the icon itself — skip
                continue;
            }

            // Check if any neighbor within `thickness` is opaque in the icon
            let mut has_opaque_neighbor = false;
            'outer: for ndy in -thickness..=thickness {
                for ndx in -thickness..=thickness {
                    let ndx_f = (dx + ndx) as f32;
                    let ndy_f = (dy + ndy) as f32;
                    let ns_x = ndx_f * cos_r + ndy_f * sin_r + cx;
                    let ns_y = -ndx_f * sin_r + ndy_f * cos_r + cy;
                    let nsx = ns_x.round() as i32;
                    let nsy = ns_y.round() as i32;
                    if nsx >= 0 && nsx < iw && nsy >= 0 && nsy < ih {
                        if icon.get_pixel(nsx as u32, nsy as u32)[3] > 128 {
                            has_opaque_neighbor = true;
                            break 'outer;
                        }
                    }
                }
            }

            if has_opaque_neighbor {
                let bg = image.get_pixel(dest_x as u32, dest_y as u32);
                let blended = Rgb([
                    (color[0] as f32 * opacity + bg[0] as f32 * (1.0 - opacity)) as u8,
                    (color[1] as f32 * opacity + bg[1] as f32 * (1.0 - opacity)) as u8,
                    (color[2] as f32 * opacity + bg[2] as f32 * (1.0 - opacity)) as u8,
                ]);
                image.put_pixel(dest_x as u32, dest_y as u32, blended);
            }
        }
    }
}

/// Draw a ship icon (pre-rasterized SVG) rotated by yaw, optionally tinted.
///
/// When `color` is `Some`, the icon's luminance is used as intensity and the result is
/// tinted to that color (for visible ships with team colors). When `None`, the icon's
/// original RGB values are used as-is (for last_visible, invisible, and dead variants
/// that have their own coloring in the SVG).
fn draw_ship_icon(
    image: &mut RgbImage,
    icon: &RgbaImage,
    x: i32,
    y: i32,
    yaw: f32,
    color: Option<Rgb<u8>>,
    opacity: f32,
) {
    let iw = icon.width() as i32;
    let ih = icon.height() as i32;
    let cx = iw as f32 / 2.0;
    let cy = ih as f32 / 2.0;
    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    // The SVG icons point upward (north = -Y in screen coords). In game coordinates,
    // yaw=0 means east (+X) and increases counter-clockwise. The screen-space rotation
    // angle R that maps icon-north (0,-1) to heading (cos(yaw), -sin(yaw)) is R = PI/2 - yaw.
    // For inverse sampling we use cos(R) and sin(R) directly:
    let cos_r = yaw.sin(); // cos(PI/2 - yaw) = sin(yaw)
    let sin_r = yaw.cos(); // sin(PI/2 - yaw) = cos(yaw)

    for dy in -ih / 2..=ih / 2 {
        for dx in -iw / 2..=iw / 2 {
            let dest_x = x + dx;
            let dest_y = y + dy;
            if dest_x < 0 || dest_x >= img_w || dest_y < 0 || dest_y >= img_h {
                continue;
            }

            // Inverse-rotate to find source pixel in the icon
            let fdx = dx as f32;
            let fdy = dy as f32;
            let src_x = fdx * cos_r + fdy * sin_r + cx;
            let src_y = -fdx * sin_r + fdy * cos_r + cy;

            let sx = src_x.round() as i32;
            let sy = src_y.round() as i32;
            if sx < 0 || sx >= iw || sy < 0 || sy >= ih {
                continue;
            }

            let pixel = icon.get_pixel(sx as u32, sy as u32);
            let alpha = pixel[3] as f32 / 255.0 * opacity;
            if alpha < 0.05 {
                continue;
            }

            let tinted = if let Some(c) = color {
                // Tint: use the icon's luminance as intensity, apply team color
                let luminance =
                    (pixel[0] as f32 * 0.299 + pixel[1] as f32 * 0.587 + pixel[2] as f32 * 0.114)
                        / 255.0;
                Rgb([
                    (c[0] as f32 * luminance) as u8,
                    (c[1] as f32 * luminance) as u8,
                    (c[2] as f32 * luminance) as u8,
                ])
            } else {
                // Use original icon colors
                Rgb([pixel[0], pixel[1], pixel[2]])
            };

            let bg = image.get_pixel(dest_x as u32, dest_y as u32);
            let blended = Rgb([
                (tinted[0] as f32 * alpha + bg[0] as f32 * (1.0 - alpha)) as u8,
                (tinted[1] as f32 * alpha + bg[1] as f32 * (1.0 - alpha)) as u8,
                (tinted[2] as f32 * alpha + bg[2] as f32 * (1.0 - alpha)) as u8,
            ]);
            image.put_pixel(dest_x as u32, dest_y as u32, blended);
        }
    }
}
