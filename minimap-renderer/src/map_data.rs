pub use wows_replays::types::WorldPos;

/// Map metadata for coordinate conversion.
#[derive(Debug, Clone)]
pub struct MapInfo {
    pub space_size: i32,
}

/// Pixel position on the minimap image.
/// (0,0) is top-left, positive X = right, positive Y = down.
/// Does NOT include HUD offset — that's applied at draw time.
#[derive(Debug, Clone, Copy)]
pub struct MinimapPos {
    pub x: i32,
    pub y: i32,
}

impl MapInfo {
    /// Convert world coordinates to minimap pixel coordinates.
    pub fn world_to_minimap(&self, pos: WorldPos, output_size: u32) -> MinimapPos {
        let scale = output_size as f64 / self.space_size as f64;
        let half = output_size as f64 / 2.0;
        MinimapPos {
            x: (pos.x as f64 * scale + half) as i32,
            y: (-pos.z as f64 * scale + half) as i32,
        }
    }
}
