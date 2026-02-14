use wowsunpack::data::idx::FileNode;
use wowsunpack::data::pkg::PkgFileLoader;
pub use wowsunpack::game_constants::{
    BattleConstants, ChannelConstants, CommonConstants, ShipsConstants, WeaponsConstants,
};

/// Composed game constants that knows which sub-constants are needed.
#[derive(Clone)]
pub struct GameConstants {
    battle: BattleConstants,
    ships: ShipsConstants,
    weapons: WeaponsConstants,
    common: CommonConstants,
    channel: ChannelConstants,
}

impl GameConstants {
    /// Load all constants from game files.
    pub fn from_pkg(file_tree: &FileNode, pkg_loader: &PkgFileLoader) -> Self {
        Self {
            battle: BattleConstants::load(file_tree, pkg_loader),
            ships: ShipsConstants::load(file_tree, pkg_loader),
            weapons: WeaponsConstants::load(file_tree, pkg_loader),
            common: CommonConstants::load(file_tree, pkg_loader),
            channel: ChannelConstants::load(file_tree, pkg_loader),
        }
    }

    /// Hardcoded defaults (no game files needed).
    pub fn defaults() -> Self {
        Self {
            battle: BattleConstants::defaults(),
            ships: ShipsConstants::defaults(),
            weapons: WeaponsConstants::defaults(),
            common: CommonConstants::defaults(),
            channel: ChannelConstants::defaults(),
        }
    }

    pub fn battle(&self) -> &BattleConstants {
        &self.battle
    }

    pub fn ships(&self) -> &ShipsConstants {
        &self.ships
    }

    pub fn weapons(&self) -> &WeaponsConstants {
        &self.weapons
    }

    pub fn common(&self) -> &CommonConstants {
        &self.common
    }

    pub fn channel(&self) -> &ChannelConstants {
        &self.channel
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

    pub fn battle_mut(&mut self) -> &mut BattleConstants {
        &mut self.battle
    }

    pub fn ships_mut(&mut self) -> &mut ShipsConstants {
        &mut self.ships
    }

    pub fn weapons_mut(&mut self) -> &mut WeaponsConstants {
        &mut self.weapons
    }

    pub fn common_mut(&mut self) -> &mut CommonConstants {
        &mut self.common
    }

    pub fn channel_mut(&mut self) -> &mut ChannelConstants {
        &mut self.channel
    }
}
