//! `ClientPlayerNameComponent` — the name shown above an entity.
//!
//! One string. This is where `SaveCharacterName` should land: sync index 65 on `Player`.

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerNameComponent {
    pub player_name: String,
}

impl PlayerNameComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "playername" => writer.write_string(&self.player_name),
            _ => return false,
        }

        true
    }
}
