use wowsunpack::data::ResourceLoader;

use crate::analyzer::decoder::DecodedPacket;

use super::BattleController;

trait BattleObserver {
    fn on_tick<G: ResourceLoader>(
        &mut self,
        controller: &BattleController<'_, '_, G>,
        event: &DecodedPacket,
    );
}
