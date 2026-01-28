use wowsunpack::data::Version;

use crate::analyzer::decoder::{DecodedPacket, DecodedPacketPayload};
use crate::packet2::Packet;
use std::collections::HashMap;
use std::convert::TryInto;

use super::analyzer::{AnalyzerMut, AnalyzerMutBuilder};

pub struct ChatLoggerBuilder;

impl Default for ChatLoggerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatLoggerBuilder {
    pub fn new() -> ChatLoggerBuilder {
        ChatLoggerBuilder
    }
}

impl AnalyzerMutBuilder for ChatLoggerBuilder {
    fn build(&self, meta: &crate::ReplayMeta) -> Box<dyn AnalyzerMut> {
        let version = Version::from_client_exe(&meta.clientVersionFromExe);
        Box::new(ChatLogger {
            usernames: HashMap::new(),
            version,
        })
    }
}

pub struct ChatLogger {
    usernames: HashMap<i32, String>,
    version: Version,
}

impl AnalyzerMut for ChatLogger {
    fn finish(&mut self) {}

    fn process_mut(&mut self, packet: &Packet<'_, '_>) {
        let decoded = DecodedPacket::from(&self.version, false, packet);
        match decoded.payload {
            DecodedPacketPayload::Chat {
                sender_id,
                audience,
                message,
                ..
            } => {
                println!(
                    "{}: {}: {} {}",
                    decoded.clock,
                    self.usernames
                        .get(&sender_id)
                        .map(String::as_str)
                        .unwrap_or("<UNKNOWN_USERNAME>"),
                    audience,
                    message
                );
            }
            DecodedPacketPayload::VoiceLine {
                sender_id, message, ..
            } => {
                println!(
                    "{}: {}: voiceline {:#?}",
                    decoded.clock,
                    self.usernames
                        .get(&sender_id)
                        .map(String::as_str)
                        .unwrap_or("<UNKNOWN_USERNAME>"),
                    message
                );
            }
            DecodedPacketPayload::OnArenaStateReceived {
                player_states: players,
                ..
            } => {
                for player in players.iter() {
                    self.usernames.insert(
                        player.meta_ship_id.try_into().unwrap(),
                        player.username.clone(),
                    );
                }
            }
            _ => {}
        }
    }
}
