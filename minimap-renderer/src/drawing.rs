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
    font: &FontRef,
) {
    let scale = PxScale::from(10.0);
    let line_height = 12i32;
    let line_count = player_name.is_some() as i32 + ship_name.is_some() as i32;
    if line_count == 0 {
        return;
    }

    // Position lines above the icon (icon radius ~12px)
    let base_y = y - 14 - line_count * line_height;
    let mut cur_y = base_y;

    if let Some(name) = player_name {
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
        draw_text_mut(image, COLOR_TEXT, tx, cur_y, scale, font, name);
        cur_y += line_height;
    }
    if let Some(name) = ship_name {
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
        draw_text_mut(image, COLOR_TEXT, tx, cur_y, scale, font, name);
    }
}

use crate::{CANVAS_HEIGHT, HUD_HEIGHT, MINIMAP_SIZE};

/// Pre-rasterized ship icon (RGBA, white/alpha mask to be tinted at draw time).
pub type ShipIcon = RgbaImage;

/// Software renderer that draws to an `RgbImage`.
///
/// Owns the map image, font, ship icons, and plane icons.
/// Implements `RenderTarget` by dispatching `DrawCommand`s to pixel-level helpers.
pub struct ImageTarget {
    canvas: RgbImage,
    /// Pre-built background: map image + grid overlay. Cloned at start of each frame.
    base_canvas: RgbImage,
    font: FontRef<'static>,
    ship_icons: HashMap<String, ShipIcon>,
    plane_icons: HashMap<String, RgbaImage>,
}

impl ImageTarget {
    pub fn new(
        map_image: Option<RgbImage>,
        ship_icons: HashMap<String, ShipIcon>,
        plane_icons: HashMap<String, RgbaImage>,
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
            } => {
                let rgb = color.map(Rgb);
                let x = pos.x;
                let y = pos.y + y_off;

                // Pick the right icon variant based on visibility and self status
                let sp = species.as_ref().expect("ship has no species");
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

                draw_ship_icon(&mut self.canvas, icon, x, y, *yaw, rgb, *opacity);
                draw_ship_labels(
                    &mut self.canvas,
                    x,
                    y,
                    player_name.as_deref(),
                    ship_name.as_deref(),
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
                player_name,
                ship_name,
            } => {
                let x = pos.x;
                let y = pos.y + y_off;
                let rgb = color.map(Rgb);

                let sp = species.as_ref().expect("dead ship has no species");
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
                draw_ship_labels(
                    &mut self.canvas,
                    x,
                    y,
                    player_name.as_deref(),
                    ship_name.as_deref(),
                    &self.font,
                );
            }
            DrawCommand::Plane { pos, icon_key } => {
                let icon = self
                    .plane_icons
                    .get(icon_key)
                    .unwrap_or_else(|| panic!("missing plane icon for '{}'", icon_key));
                draw_plane_icon(&mut self.canvas, icon, pos.x, pos.y + y_off);
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
