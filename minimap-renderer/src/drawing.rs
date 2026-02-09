use ab_glyph::{FontRef, PxScale};
use image::{Rgb, RgbImage};
use imageproc::drawing::{
    draw_filled_circle_mut, draw_filled_rect_mut, draw_line_segment_mut, draw_text_mut,
};
use imageproc::rect::Rect;
use wows_replays::analyzer::battle_controller::Relation;

// Ship colors by relation
pub const COLOR_SELF: Rgb<u8> = Rgb([255, 255, 255]);
pub const COLOR_ALLY: Rgb<u8> = Rgb([0, 200, 0]);
pub const COLOR_ENEMY: Rgb<u8> = Rgb([255, 60, 60]);
pub const COLOR_DEAD: Rgb<u8> = Rgb([128, 128, 128]);

pub const COLOR_TORPEDO: Rgb<u8> = Rgb([255, 80, 80]);
pub const COLOR_SHOT: Rgb<u8> = Rgb([255, 200, 50]);
pub const COLOR_PLANE: Rgb<u8> = Rgb([100, 180, 255]);

pub const COLOR_TEAM_GREEN: Rgb<u8> = Rgb([0, 180, 0]);
pub const COLOR_TEAM_RED: Rgb<u8> = Rgb([200, 0, 0]);

pub const COLOR_TEXT: Rgb<u8> = Rgb([255, 255, 255]);
pub const COLOR_TEXT_SHADOW: Rgb<u8> = Rgb([0, 0, 0]);

const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

pub fn load_font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_DATA).expect("failed to load embedded font")
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
pub fn draw_torpedo(image: &mut RgbImage, x: i32, y: i32) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < 0 || x >= w || y < 0 || y >= h {
        return;
    }
    draw_filled_circle_mut(image, (x, y), 2, COLOR_TORPEDO);
}

/// Draw a plane dot.
pub fn draw_plane(image: &mut RgbImage, x: i32, y: i32) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    if x < 0 || x >= w || y < 0 || y >= h {
        return;
    }
    draw_filled_circle_mut(image, (x, y), 2, COLOR_PLANE);
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
