use wows_replays::analyzer::decoder::DeathCause;

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

/// Kind of ship configuration circle for filtering and grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipConfigCircleKind {
    Detection,
    MainBattery,
    SecondaryBattery,
    Radar,
    Hydro,
}

/// A single chat message entry for the chat overlay.
#[derive(Debug, Clone)]
pub struct ChatEntry {
    /// Clan tag (e.g. "CLAN"), empty if none
    pub clan_tag: String,
    /// Clan color as RGB, or None to use team color
    pub clan_color: Option<[u8; 3]>,
    /// Player name
    pub player_name: String,
    /// Team color for the player name
    pub team_color: [u8; 3],
    /// Ship species for icon lookup (e.g. "Destroyer")
    pub ship_species: Option<String>,
    /// Localized ship name (e.g. "Shimakaze")
    pub ship_name: Option<String>,
    /// Chat message text
    pub message: String,
    /// Color for the message text (reflects the chat channel)
    pub message_color: [u8; 3],
    /// Opacity (0.0 = fully faded, 1.0 = fully visible)
    pub opacity: f32,
}

/// A single entry in the kill feed.
#[derive(Debug, Clone)]
pub struct KillFeedEntry {
    /// Killer's player name
    pub killer_name: String,
    /// Killer's ship species (e.g. "Destroyer") for icon lookup
    pub killer_species: Option<String>,
    /// Killer's localized ship name (e.g. "Shimakaze")
    pub killer_ship_name: Option<String>,
    /// Killer's team color
    pub killer_color: [u8; 3],
    /// Victim's player name
    pub victim_name: String,
    /// Victim's ship species for icon lookup
    pub victim_species: Option<String>,
    /// Victim's localized ship name
    pub victim_ship_name: Option<String>,
    /// Victim's team color
    pub victim_color: [u8; 3],
    /// How the victim died
    pub cause: DeathCause,
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
        /// Override color for player name based on selected armament
        /// (e.g. orange=HE, light blue=AP, green=torp). None = default white.
        name_color: Option<[u8; 3]>,
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
    /// Arms race buff zone circle
    BuffZone {
        pos: MinimapPos,
        /// Zone radius in pixels
        radius: i32,
        /// Team color (green/red/white)
        color: [u8; 3],
        /// Fill transparency
        alpha: f32,
        /// Marker name for icon lookup (e.g. "damage_active")
        marker_name: Option<String>,
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
    /// Fighter patrol radius circle (filled only, no outline)
    PatrolRadius {
        pos: MinimapPos,
        /// Radius in pixels
        radius_px: i32,
        /// Circle color (team-colored)
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
    /// Ship configuration range circle (detection, main battery, secondary, radar, hydro)
    ShipConfigCircle {
        pos: MinimapPos,
        /// Radius in minimap pixels
        radius_px: f32,
        color: [u8; 3],
        alpha: f32,
        /// Whether circle should be dashed (detection) or solid
        dashed: bool,
        /// Label text (e.g. "12.0 km")
        label: Option<String>,
        kind: ShipConfigCircleKind,
        /// Player name for filtering per-ship
        player_name: String,
        /// Whether this is the replay player's own ship
        is_self: bool,
    },
    /// Position trail showing historical movement as colored dots
    PositionTrail {
        /// Player name for filtering trails per-ship
        player_name: Option<String>,
        /// Points with interpolated colors (oldest=blue, newest=red)
        points: Vec<(MinimapPos, [u8; 3])>,
    },
    /// Team buff indicators below the score bar (arms race)
    TeamBuffs {
        /// Friendly team buffs: (marker_name, count), sorted by sorting field
        friendly_buffs: Vec<(String, u32)>,
        /// Enemy team buffs: (marker_name, count), sorted by sorting field
        enemy_buffs: Vec<(String, u32)>,
    },
    /// Score bar
    ScoreBar {
        team0: i32,
        team1: i32,
        team0_color: [u8; 3],
        team1_color: [u8; 3],
        /// Win score threshold (from BattleLogic, typically 1000)
        max_score: i32,
        /// Time-to-win for team 0 (e.g. "5:32"), or None if no caps
        team0_timer: Option<String>,
        /// Time-to-win for team 1 (e.g. "3:15"), or None if no caps
        team1_timer: Option<String>,
    },
    /// Team advantage indicator (shown in score bar area)
    TeamAdvantage {
        /// Advantage label (e.g. "Strong", "Moderate"), empty if Even
        label: String,
        /// Color for the label (advantaged team's color)
        color: [u8; 3],
        /// Detailed breakdown for tooltip display
        breakdown: crate::advantage::AdvantageBreakdown,
    },
    /// Game timer
    Timer { seconds: f32 },
    /// Kill feed entries with rich data
    KillFeed { entries: Vec<KillFeedEntry> },
    /// Chat overlay on the left side of the minimap
    ChatOverlay { entries: Vec<ChatEntry> },
    /// Battle result overlay (shown at end of match)
    BattleResultOverlay {
        text: String,
        /// Subtitle (e.g. finish reason like "All enemy ships destroyed")
        subtitle: Option<String>,
        /// Glow/shadow color behind the text
        color: [u8; 3],
    },
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
