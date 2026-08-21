//! Print the wire bytes for each `CharcterCreationResponse`.
//!
//! Used to drive the reply through another transport while the Rust game server does not
//! yet exist — the bytes are this crate's, whoever sends them.
//!
//!   cargo run -p skysaga-proto --example emit-response

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::CharacterCreationResponse;

fn main() {
    for response in [
        CharacterCreationResponse::CharacterSaved,
        CharacterCreationResponse::CharacterSaveFailed,
        CharacterCreationResponse::HomeworldCreated,
        CharacterCreationResponse::HomeworldCreationFailed,
    ] {
        let mut writer = BitWriter::new();
        response.encode(&mut writer);

        let hex: String = writer.as_bytes().iter().map(|b| format!("{b:02X}")).collect();

        println!(
            "{:<24} value {}  {} bits  {} bytes  {hex}",
            format!("{response:?}"),
            response.value(),
            writer.bits_used(),
            writer.as_bytes().len(),
        );
    }
}
