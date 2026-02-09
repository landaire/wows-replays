pub use wows_replays::packet2::GameClock;

/// Map metadata for coordinate conversion.
#[derive(Debug, Clone)]
pub struct MapInfo {
    pub space_size: i32,
}

/// World position in BigWorld units (game engine coordinates).
/// X = east/west, Z = north/south. Origin at map center.
#[derive(Debug, Clone, Copy)]
pub struct WorldPos {
    pub x: f32,
    pub z: f32,
}

/// Pixel position on the minimap image.
/// (0,0) is top-left, positive X = right, positive Y = down.
/// Does NOT include HUD offset — that's applied at draw time.
#[derive(Debug, Clone, Copy)]
pub struct MinimapPos {
    pub x: i32,
    pub y: i32,
}

/// Normalized minimap position from MinimapUpdate packets.
/// Values in [0,1] range. (0,0) = top-left, (1,1) = bottom-right.
/// Note: in game data, y=0 is bottom, y=1 is top, so we flip at conversion.
#[derive(Debug, Clone, Copy)]
pub struct NormalizedPos {
    pub x: f32,
    pub y: f32,
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

impl NormalizedPos {
    /// Convert normalized [0,1] position to minimap pixel coordinates.
    /// Flips Y axis: game's y=0 is bottom, minimap y=0 is top.
    pub fn to_minimap(self, output_size: u32) -> MinimapPos {
        MinimapPos {
            x: (self.x * output_size as f32) as i32,
            y: ((1.0 - self.y) * output_size as f32) as i32,
        }
    }
}
