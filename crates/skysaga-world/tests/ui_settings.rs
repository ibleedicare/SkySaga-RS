//! `clientuisettingscomponent`, against the client's reader.
//!
//! Every width here is read from `UISettingsComponent_ReadSync` (`FUN_008d4480`) and the
//! functions it calls; see `documentations/ui-settings-and-hotbar.md`. STAT, not yet LIVE.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::crafting::ItemSpec;
use skysaga_world::{Component, HotbarSlot, UiSettingsComponent};

fn synced(component: &UiSettingsComponent, parameter: &str) -> (bool, BitWriter) {
    let mut writer = BitWriter::new();
    let wrote = Component::UiSettings(component.clone()).sync(parameter, &mut writer);

    (wrote, writer)
}

fn dirt() -> ItemSpec {
    ItemSpec {
        resource: Some(skysaga_core::name_hash("Dirt")),
        ..ItemSpec::default()
    }
}

/// An empty `ItemSpec`: absent resource, a 3-bit count of 0, absent teach item, empty string.
const EMPTY_SPEC_BITS: usize = 1 + 3 + 1 + 1;

#[test]
fn the_component_is_named_as_the_entities_file_names_it() {
    assert_eq!(
        Component::UiSettings(UiSettingsComponent::default()).name(),
        "clientuisettingscomponent",
    );
}

#[test]
fn a_new_hotbar_is_eight_empty_squares_behind_one_clear_bit() {
    // ReadHotbarSlotList (FUN_008d4170): the count is ranged 8..8, so 0 bits, then one bit
    // that is clear when exactly eight entries follow. Each entry is a 32-bit word and two
    // specs (FUN_008d3e80).
    let (wrote, writer) = synced(&UiSettingsComponent::default(), "hotbarslotresources");

    assert!(wrote);
    assert_eq!(writer.bits_used(), 1 + 8 * (32 + 2 * EMPTY_SPEC_BITS));

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert!(!reader.read_bit().unwrap(), "eight entries take the short path");
}

#[test]
fn a_bound_square_carries_its_spec_in_the_hand_it_was_bound_to() {
    let mut component = UiSettingsComponent::default();
    component.hotbar[2].hands[1] = dirt();

    let (_, writer) = synced(&component, "hotbarslotresources");
    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    reader.read_bit().unwrap();

    for square in 0..8 {
        assert_eq!(reader.read_u32().unwrap(), 0, "square {square}: the leading word");

        let hands = [
            ItemSpec::decode(&mut reader).unwrap(),
            ItemSpec::decode(&mut reader).unwrap(),
        ];

        let expected = if square == 2 {
            [ItemSpec::default(), dirt()]
        } else {
            [ItemSpec::default(), ItemSpec::default()]
        };

        assert_eq!(hands, expected, "square {square}");
    }
}

#[test]
fn a_hotbar_of_another_length_spells_its_count_out() {
    let component = UiSettingsComponent {
        hotbar: vec![HotbarSlot::default(); 3],
        active_slot: 0,
    };

    let (_, writer) = synced(&component, "hotbarslotresources");
    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert!(reader.read_bit().unwrap());
    assert_eq!(reader.read_u32().unwrap(), 3);
}

#[test]
fn the_active_slot_is_three_bits() {
    // FUN_008d3b00: 8 - NumBitsRequired8(7) = 3 bits.
    let component = UiSettingsComponent {
        active_slot: 5,
        ..UiSettingsComponent::default()
    };

    let (wrote, writer) = synced(&component, "activeslot");

    assert!(wrote);
    assert_eq!(writer.bits_used(), 3);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_bits_le(3).unwrap(), 5);
}

#[test]
fn an_active_slot_past_the_end_is_clamped_as_the_client_clamps_it() {
    let component = UiSettingsComponent {
        active_slot: 12,
        ..UiSettingsComponent::default()
    };

    let (_, writer) = synced(&component, "activeslot");
    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_bits_le(3).unwrap(), 7);
}

#[test]
fn another_parameter_is_not_this_components() {
    let (wrote, writer) = synced(&UiSettingsComponent::default(), "position");

    assert!(!wrote);
    assert_eq!(writer.bits_used(), 0);
}
