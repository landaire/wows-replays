use serde::{Deserialize, Serialize};

use crate::renderer::RenderOptions;

/// Renderer configuration, loadable from a TOML file.
///
/// All fields default to their standard values. CLI flags override config file values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RendererConfig {
    // Display toggles (all default true)
    pub show_player_names: bool,
    pub show_ship_names: bool,
    pub show_capture_points: bool,
    pub show_buildings: bool,
    pub show_turret_direction: bool,
    pub show_hp_bars: bool,
    pub show_tracers: bool,
    pub show_torpedoes: bool,
    pub show_planes: bool,
    pub show_smoke: bool,
    pub show_score: bool,
    pub show_timer: bool,
    pub show_kill_feed: bool,
    pub show_consumables: bool,
    // New features (default false)
    pub show_armament: bool,
    pub show_trails: bool,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            show_player_names: true,
            show_ship_names: true,
            show_capture_points: true,
            show_buildings: true,
            show_turret_direction: true,
            show_hp_bars: true,
            show_tracers: true,
            show_torpedoes: true,
            show_planes: true,
            show_smoke: true,
            show_score: true,
            show_timer: true,
            show_kill_feed: true,
            show_consumables: true,
            show_armament: false,
            show_trails: false,
        }
    }
}

impl RendererConfig {
    /// Load config from a TOML file.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Convert into RenderOptions for the renderer.
    pub fn into_render_options(self) -> RenderOptions {
        RenderOptions {
            show_player_names: self.show_player_names,
            show_ship_names: self.show_ship_names,
            show_capture_points: self.show_capture_points,
            show_buildings: self.show_buildings,
            show_turret_direction: self.show_turret_direction,
            show_hp_bars: self.show_hp_bars,
            show_tracers: self.show_tracers,
            show_torpedoes: self.show_torpedoes,
            show_planes: self.show_planes,
            show_smoke: self.show_smoke,
            show_score: self.show_score,
            show_timer: self.show_timer,
            show_kill_feed: self.show_kill_feed,
            show_consumables: self.show_consumables,
            show_armament: self.show_armament,
            show_trails: self.show_trails,
        }
    }

    /// Generate a commented default TOML config string.
    pub fn generate_default_toml() -> String {
        r#"# Minimap Renderer Configuration
# Place this file as minimap_renderer.toml next to the executable,
# or specify with --config <path>.

# Display toggles (true = show, false = hide)

# Show player names above ship icons
show_player_names = true

# Show ship type names above ship icons
show_ship_names = true

# Show capture point zones with progress
show_capture_points = true

# Show building markers (e.g. shipyard structures)
show_buildings = true

# Show turret direction indicators
show_turret_direction = true

# Show health bars below ship icons
show_hp_bars = true

# Show artillery shell tracers
show_tracers = true

# Show torpedo markers
show_torpedoes = true

# Show plane squadron icons
show_planes = true

# Show smoke screen clouds
show_smoke = true

# Show team score bar at top
show_score = true

# Show game timer
show_timer = true

# Show kill feed in top-right corner
show_kill_feed = true

# Show active consumable icons below ships
show_consumables = true

# Show selected armament/ammo type below ship icons (e.g. AP, HE, SAP, Torp)
show_armament = false

# Show position trail heatmap (rainbow: blue=oldest, red=newest)
show_trails = false
"#
        .to_string()
    }

    /// Apply CLI flag overrides. Flags use negative form (--no-X disables, --show-X enables).
    pub fn apply_cli_overrides(&mut self, matches: &clap::ArgMatches) {
        if matches.is_present("NO_PLAYER_NAMES") {
            self.show_player_names = false;
        }
        if matches.is_present("NO_SHIP_NAMES") {
            self.show_ship_names = false;
        }
        if matches.is_present("NO_CAPTURE_POINTS") {
            self.show_capture_points = false;
        }
        if matches.is_present("NO_BUILDINGS") {
            self.show_buildings = false;
        }
        if matches.is_present("NO_TURRET_DIRECTION") {
            self.show_turret_direction = false;
        }
        if matches.is_present("SHOW_ARMAMENT") {
            self.show_armament = true;
        }
        if matches.is_present("SHOW_TRAILS") {
            self.show_trails = true;
        }
    }
}
