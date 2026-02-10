use ab_glyph::{FontRef, PxScale};
use image::{Rgb, RgbImage, RgbaImage};
use imageproc::drawing::{
    draw_filled_circle_mut, draw_filled_rect_mut, draw_line_segment_mut, draw_text_mut,
};
use imageproc::rect::Rect;
use wows_replays::types::Relation;

// Ship colors by relation (matches in-game minimap: #ffffff, #4ce8aa, #fe4d2a)
pub const COLOR_SELF: Rgb<u8> = Rgb([255, 255, 255]);
pub const COLOR_ALLY: Rgb<u8> = Rgb([76, 232, 170]);
pub const COLOR_ENEMY: Rgb<u8> = Rgb([254, 77, 42]);
pub const COLOR_DEAD: Rgb<u8> = Rgb([128, 128, 128]);

pub const COLOR_TORPEDO: Rgb<u8> = Rgb([254, 77, 42]);
pub const COLOR_TORPEDO_FRIENDLY: Rgb<u8> = Rgb([76, 232, 170]);
pub const COLOR_SHOT: Rgb<u8> = Rgb([255, 200, 50]);

pub const COLOR_TEAM_GREEN: Rgb<u8> = Rgb([76, 232, 170]);
pub const COLOR_TEAM_RED: Rgb<u8> = Rgb([254, 77, 42]);

pub const COLOR_TEXT: Rgb<u8> = Rgb([255, 255, 255]);
pub const COLOR_TEXT_SHADOW: Rgb<u8> = Rgb([0, 0, 0]);

const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

pub fn load_font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_DATA).expect("failed to load embedded font")
}

/// How a ship should be rendered based on its visibility state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShipVisibility {
    /// Ship is directly visible (Position packets). Solid fill.
    Visible,
    /// Ship is detected on minimap but not directly rendered. Outline only.
    MinimapOnly,
    /// Ship has gone undetected. Gray, semi-transparent at last known position.
    Undetected,
}

/// Draw a ship as a filled circle with a heading line.
pub fn draw_ship(image: &mut RgbImage, x: i32, y: i32, yaw: f32, color: Rgb<u8>, radius: i32) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < -radius || x >= w + radius || y < -radius || y >= h + radius {
        return;
    }
    draw_filled_circle_mut(image, (x, y), radius, color);

    // Heading line
    let line_len = (radius + 4) as f32;
    let end_x = x as f32 + yaw.cos() * line_len;
    let end_y = y as f32 - yaw.sin() * line_len;
    draw_line_segment_mut(image, (x as f32, y as f32), (end_x, end_y), color);
}

/// Draw a ship circle as outline only (ring + heading line).
pub fn draw_ship_outline(
    image: &mut RgbImage,
    x: i32,
    y: i32,
    yaw: f32,
    color: Rgb<u8>,
    radius: i32,
) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < -radius || x >= w + radius || y < -radius || y >= h + radius {
        return;
    }

    let r2_outer = (radius * radius) as f32;
    let r2_inner = ((radius - 1).max(0) * (radius - 1).max(0)) as f32;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let d2 = (dx * dx + dy * dy) as f32;
            if d2 <= r2_outer && d2 >= r2_inner {
                let px = x + dx;
                let py = y + dy;
                if px >= 0 && px < w && py >= 0 && py < h {
                    image.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }

    let line_len = (radius + 4) as f32;
    let end_x = x as f32 + yaw.cos() * line_len;
    let end_y = y as f32 - yaw.sin() * line_len;
    draw_line_segment_mut(image, (x as f32, y as f32), (end_x, end_y), color);
}

/// Draw a ship as a gray, semi-transparent circle (for undetected ships, no icon fallback).
pub fn draw_ship_undetected(image: &mut RgbImage, x: i32, y: i32, yaw: f32, radius: i32) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < -radius || x >= w + radius || y < -radius || y >= h + radius {
        return;
    }

    let opacity = 0.4f32;
    let r2 = (radius * radius) as f32;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if (dx * dx + dy * dy) as f32 > r2 {
                continue;
            }
            let px = x + dx;
            let py = y + dy;
            if px >= 0 && px < w && py >= 0 && py < h {
                let bg = image.get_pixel(px as u32, py as u32);
                let blended = Rgb([
                    (COLOR_DEAD[0] as f32 * opacity + bg[0] as f32 * (1.0 - opacity)) as u8,
                    (COLOR_DEAD[1] as f32 * opacity + bg[1] as f32 * (1.0 - opacity)) as u8,
                    (COLOR_DEAD[2] as f32 * opacity + bg[2] as f32 * (1.0 - opacity)) as u8,
                ]);
                image.put_pixel(px as u32, py as u32, blended);
            }
        }
    }

    let line_len = (radius + 4) as f32;
    let end_x = x as f32 + yaw.cos() * line_len;
    let end_y = y as f32 - yaw.sin() * line_len;
    draw_line_segment_mut(image, (x as f32, y as f32), (end_x, end_y), COLOR_DEAD);
}

/// Draw a dead ship marker (X shape).
pub fn draw_dead_ship(image: &mut RgbImage, x: i32, y: i32) {
    let size = 4.0f32;
    draw_line_segment_mut(
        image,
        (x as f32 - size, y as f32 - size),
        (x as f32 + size, y as f32 + size),
        COLOR_DEAD,
    );
    draw_line_segment_mut(
        image,
        (x as f32 + size, y as f32 - size),
        (x as f32 - size, y as f32 + size),
        COLOR_DEAD,
    );
}

/// Draw an artillery shot trajectory line.
pub fn draw_shot_line(image: &mut RgbImage, x1: f32, y1: f32, x2: f32, y2: f32) {
    draw_line_segment_mut(image, (x1, y1), (x2, y2), COLOR_SHOT);
}

/// Draw a torpedo dot.
pub fn draw_torpedo(image: &mut RgbImage, x: i32, y: i32, color: Rgb<u8>) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < 0 || x >= w || y < 0 || y >= h {
        return;
    }
    draw_filled_circle_mut(image, (x, y), 2, color);
}

/// Draw a smoke screen as a semi-transparent filled circle.
pub fn draw_smoke(image: &mut RgbImage, x: i32, y: i32, radius: i32) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    let smoke_color: [u8; 3] = [180, 180, 180];
    let alpha = 0.3f32;

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
pub fn draw_plane_icon(image: &mut RgbImage, icon: &RgbaImage, x: i32, y: i32) {
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

/// Draw a plane as a fallback colored dot.
pub fn draw_plane_dot(image: &mut RgbImage, x: i32, y: i32, color: Rgb<u8>) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < 0 || x >= w || y < 0 || y >= h {
        return;
    }
    draw_filled_circle_mut(image, (x, y), 2, color);
}

/// Draw the team score bar at the top of the frame.
pub fn draw_score_bar(image: &mut RgbImage, team0_score: i32, team1_score: i32, font: &FontRef) {
    let width = image.width();
    let bar_height = 20u32;
    let total = (team0_score + team1_score).max(1) as f32;
    let green_width = ((team0_score as f32 / total) * width as f32) as u32;

    // Green bar (team 0 / allies)
    if green_width > 0 {
        draw_filled_rect_mut(
            image,
            Rect::at(0, 0).of_size(green_width, bar_height),
            COLOR_TEAM_GREEN,
        );
    }
    // Red bar (team 1 / enemies)
    if green_width < width {
        draw_filled_rect_mut(
            image,
            Rect::at(green_width as i32, 0).of_size(width - green_width, bar_height),
            COLOR_TEAM_RED,
        );
    }

    // Score text
    let scale = PxScale::from(14.0);
    let green_text = format!("{}", team0_score);
    let red_text = format!("{}", team1_score);
    draw_text_mut(image, COLOR_TEXT, 5, 3, scale, font, &green_text);
    let red_x = width as i32 - (red_text.len() as i32 * 9) - 5;
    draw_text_mut(image, COLOR_TEXT, red_x, 3, scale, font, &red_text);
}

/// Draw the game timer.
pub fn draw_timer(image: &mut RgbImage, game_time_secs: f32, font: &FontRef) {
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
pub fn draw_kill_feed(
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
pub fn draw_grid(image: &mut RgbImage, minimap_size: u32, y_off: u32, font: &FontRef) {
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
/// The bar is colored green→yellow→red based on health remaining.
pub fn draw_health_bar(image: &mut RgbImage, x: i32, y: i32, fraction: f32) {
    let bar_w = 20i32;
    let bar_h = 3i32;
    let bar_x = x - bar_w / 2;
    let bar_y = y + 10; // below the ship icon

    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    let fill_w = (fraction.clamp(0.0, 1.0) * bar_w as f32).round() as i32;

    // Health color: green at full, yellow at half, red at low
    let fill_color = if fraction > 0.5 {
        let t = (fraction - 0.5) * 2.0;
        Rgb([((1.0 - t) * 255.0) as u8, 255, 0])
    } else {
        let t = fraction * 2.0;
        Rgb([255, (t * 255.0) as u8, 0])
    };

    // Background (dark)
    let bg_color = Rgb([40u8, 40, 40]);
    let bg_alpha = 0.6f32;

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

/// Get the ship color based on relation to the recording player.
pub fn ship_color(relation: Relation) -> Rgb<u8> {
    if relation.is_self() {
        COLOR_SELF
    } else if relation.is_ally() {
        COLOR_ALLY
    } else {
        COLOR_ENEMY
    }
}

/// Draw a ship icon (pre-rasterized SVG) rotated by yaw and tinted with the given color.
///
/// The icon is expected to be a white/alpha mask -- non-transparent pixels are tinted to `color`.
/// The icon is rotated about its center by `yaw` radians (game convention: 0=east, CCW positive).
/// The `visibility` parameter controls the rendering style.
pub fn draw_ship_icon(
    image: &mut RgbImage,
    icon: &RgbaImage,
    x: i32,
    y: i32,
    yaw: f32,
    color: Rgb<u8>,
    visibility: ShipVisibility,
) {
    let iw = icon.width() as i32;
    let ih = icon.height() as i32;
    let cx = iw as f32 / 2.0;
    let cy = ih as f32 / 2.0;
    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    let (draw_color, opacity) = match visibility {
        ShipVisibility::Visible => (color, 1.0f32),
        ShipVisibility::MinimapOnly => (color, 1.0),
        ShipVisibility::Undetected => (COLOR_DEAD, 0.4),
    };

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

            // Tint: use the icon's luminance as intensity, apply team color
            let luminance =
                (pixel[0] as f32 * 0.299 + pixel[1] as f32 * 0.587 + pixel[2] as f32 * 0.114)
                    / 255.0;

            // For outline mode, only draw edge pixels (high alpha neighbors indicate interior)
            if visibility == ShipVisibility::MinimapOnly {
                let is_edge = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(ox, oy)| {
                    let nx = sx + ox;
                    let ny = sy + oy;
                    if nx < 0 || nx >= iw || ny < 0 || ny >= ih {
                        return true;
                    }
                    icon.get_pixel(nx as u32, ny as u32)[3] < 128
                });
                if !is_edge {
                    continue;
                }
            }

            let tinted = Rgb([
                (draw_color[0] as f32 * luminance) as u8,
                (draw_color[1] as f32 * luminance) as u8,
                (draw_color[2] as f32 * luminance) as u8,
            ]);

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
