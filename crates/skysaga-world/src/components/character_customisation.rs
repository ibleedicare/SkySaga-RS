//! `ClientCharacterCustomisationComponent` — what the player's character looks like.
//!
//! One parameter, `customisationdata`, at sync index 19 on `Player`. It carries the gender,
//! tribe, skin/eye/clothing colours and hairstyle chosen in the in-game creator, and it is
//! how the client learns the appearance of *any* character including its own.
//!
//! # Why this was missing
//!
//! The C# emulator resolves component classes by reflection over the names in
//! `Entities.json`, and a name with no matching class is skipped with a `Debug.WriteLine`
//! that is invisible in a release build. No `ClientCharacterCustomisationComponent` class was
//! ever written, so sync index 19 silently never replicated: the server received
//! `SetCharacterCustomisationData` from the client, stored it, and never told anyone. Every
//! character therefore rendered with the client's built-in defaults regardless of what the
//! player picked in the creator — the appearance round-tripped nowhere.
//!
//! Here the component is a variant on an exhaustive enum, so the equivalent omission is a
//! compile error rather than silence.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::customisation::CustomisationData;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CharacterCustomisationComponent {
    pub customisation: CustomisationData,
}

impl CharacterCustomisationComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            // Written verbatim: the parameter *is* a CustomisationData, with no framing of
            // its own. Its length is implied by the schema, not prefixed.
            "customisationdata" => self.customisation.encode(writer),
            _ => return false,
        }

        true
    }
}
