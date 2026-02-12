use wowsunpack::data::idx::FileNode;
use wowsunpack::data::pkg::PkgFileLoader;
pub use wowsunpack::game_constants::{BattleConstants, ShipsConstants};

/// Composed game constants that knows which sub-constants are needed.
pub struct GameConstants {
    battle: BattleConstants,
    ships: ShipsConstants,
}

impl GameConstants {
    /// Load all constants from game files.
    pub fn from_pkg(file_tree: &FileNode, pkg_loader: &PkgFileLoader) -> Self {
        Self {
            battle: BattleConstants::load(file_tree, pkg_loader),
            ships: ShipsConstants::load(file_tree, pkg_loader),
        }
    }

    /// Hardcoded defaults (no game files needed).
    pub fn defaults() -> Self {
        Self {
            battle: BattleConstants::defaults(),
            ships: ShipsConstants::defaults(),
        }
    }

    pub fn battle(&self) -> &BattleConstants {
        &self.battle
    }

    pub fn ships(&self) -> &ShipsConstants {
        &self.ships
    }

    pub fn game_mode_name(&self, id: i32) -> Option<&str> {
        self.battle.game_mode(id)
    }

    pub fn death_reason_name(&self, id: i32) -> Option<&str> {
        self.battle.death_reason(id)
    }

    pub fn camera_mode_name(&self, id: i32) -> Option<&str> {
        self.battle.camera_mode(id)
    }
}
