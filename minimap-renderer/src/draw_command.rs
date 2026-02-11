use crate::map_data::MinimapPos;

/// How a ship should be rendered based on its visibility state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShipVisibility {
    /// Ship is directly visible (Position packets). Solid fill.
    Visible,
    /// Ship is detected on minimap but not directly rendered. Outline only.
    MinimapOnly,
    /// Ship has gone undetected. Gray, semi-transparent at last known position.
    Undetected,
}

/// A high-level draw command emitted by the renderer.
///
/// The renderer reads game state and produces a sequence of these commands.
/// A `RenderTarget` implementation consumes them to produce visual output,
/// whether that's a software-rendered image or GPU draw calls.
///
/// All visual properties (colors, opacity, etc.) are fully resolved by the renderer,
/// so backends don't need to duplicate game logic.
#[derive(Debug)]
pub enum DrawCommand {
    /// Artillery tracer line segment
    ShotTracer {
        from: MinimapPos,
        to: MinimapPos,
        color: [u8; 3],
    },
    /// Torpedo dot
    Torpedo { pos: MinimapPos, color: [u8; 3] },
    /// Smoke puff circle (alpha blended)
    Smoke {
        pos: MinimapPos,
        radius: i32,
        color: [u8; 3],
        alpha: f32,
    },
    /// Ship with icon, rotation, color, visibility
    Ship {
        pos: MinimapPos,
        yaw: f32,
        /// Species name for icon lookup (e.g. "Destroyer")
        species: Option<String>,
        /// Tint color. None = use the icon's native colors (for last_visible/invisible variants)
        color: Option<[u8; 3]>,
        visibility: ShipVisibility,
        opacity: f32,
        /// Whether this is the player's own ship (uses `_self` icon variant)
        is_self: bool,
        /// Player name to render above the icon
        player_name: Option<String>,
        /// Localized ship name to render above the icon (below player name)
        ship_name: Option<String>,
        /// Whether this ship is a detected teammate (ally visible but not self)
        is_detected_teammate: bool,
    },
    /// Health bar above a ship
    HealthBar {
        pos: MinimapPos,
        fraction: f32,
        fill_color: [u8; 3],
        background_color: [u8; 3],
        background_alpha: f32,
    },
    /// Dead ship marker
    DeadShip {
        pos: MinimapPos,
        yaw: f32,
        species: Option<String>,
        /// Tint color. None = use the icon's native colors
        color: Option<[u8; 3]>,
        is_self: bool,
        /// Player name to render above the icon
        player_name: Option<String>,
        /// Localized ship name to render above the icon (below player name)
        ship_name: Option<String>,
    },
    /// Capture zone circle with team coloring and letter label
    CapturePoint {
        pos: MinimapPos,
        /// Zone radius in pixels
        radius: i32,
        /// Team color (green/red/white) for the owning team
        color: [u8; 3],
        /// Fill transparency
        alpha: f32,
        /// Zone label (e.g. "A", "B", "C")
        label: String,
        /// Capture progress 0.0..1.0 (0 = no capture in progress)
        progress: f32,
        /// Color of the invading team (shown as progress arc)
        invader_color: Option<[u8; 3]>,
    },
    /// Turret direction indicator line from ship center
    TurretDirection {
        pos: MinimapPos,
        /// Turret yaw in radians (world-space, already includes ship heading)
        yaw: f32,
        color: [u8; 3],
        /// Line length in pixels
        length: i32,
    },
    /// Building dot on the minimap
    Building {
        pos: MinimapPos,
        color: [u8; 3],
        is_alive: bool,
    },
    /// Plane icon
    Plane {
        pos: MinimapPos,
        /// Icon key for lookup (e.g. "controllable/fighter_he_enemy")
        icon_key: String,
    },
    /// Consumable detection radius circle (radar, hydro, etc.)
    ConsumableRadius {
        pos: MinimapPos,
        /// Radius in pixels
        radius_px: i32,
        /// Circle color (team-colored: green for friendly, red for enemy)
        color: [u8; 3],
        /// Fill transparency
        alpha: f32,
    },
    /// Active consumable icons laid out horizontally below a ship
    ConsumableIcons {
        pos: MinimapPos,
        /// Icon keys for lookup (e.g. "PCY019_RLSSearch")
        icon_keys: Vec<String>,
        /// True for self/allies, false for enemies (affects tint color)
        is_friendly: bool,
        /// Whether a health bar is rendered below this ship (affects vertical offset)
        has_hp_bar: bool,
    },
    /// Score bar
    ScoreBar {
        team0: i32,
        team1: i32,
        team0_color: [u8; 3],
        team1_color: [u8; 3],
    },
    /// Game timer
    Timer { seconds: f32 },
    /// Kill feed entries (killer_name, victim_name)
    KillFeed { entries: Vec<(String, String)> },
}

/// Trait for rendering backends that consume `DrawCommand`s.
///
/// Implementations produce visual output from high-level draw commands.
/// The software image renderer and a future GPU renderer both implement this.
pub trait RenderTarget {
    /// Prepare a fresh frame (clear canvas, draw background map + grid).
    fn begin_frame(&mut self);

    /// Execute a single draw command.
    fn draw(&mut self, cmd: &DrawCommand);

    /// Finalize the current frame. After this call, the frame is ready to read/encode.
    fn end_frame(&mut self);
}
