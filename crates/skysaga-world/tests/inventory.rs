//! The inventory model: what a drag, a split, a merge, a trash and a "take all" actually do.
//!
//! Pure, so none of this needs a socket, a client or a running server. The behaviour asserted
//! is the C#'s, which is the oracle; where this port deliberately differs there is a test
//! saying so and why.

use skysaga_world::inventory::{Effect, Inventories, StackLimits};

/// Two item names, as the hashes the wire actually carries.
fn dirt() -> u32 {
    skysaga_core::name_hash("Dirt")
}

fn stone() -> u32 {
    skysaga_core::name_hash("Stone")
}

/// A player with an empty 45-slot inventory, and nothing else in the world.
fn player() -> (Inventories, u32) {
    let mut inventories = Inventories::new(StackLimits::default(), 1000);

    let player = 10;

    inventories.open(player, 45);

    (inventories, player)
}

// --- moving a whole stack --------------------------------------------------------------

#[test]
fn a_whole_stack_moves_to_an_empty_slot() {
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 10).unwrap();

    let effects = inventories.transfer_to_slot(player, 9, player, 12, 10);

    assert_eq!(inventories.slot(player, 9), Some(0));
    assert_eq!(inventories.slot(player, 12), Some(item));

    // The item entity is untouched: only the owner's slot list changed, so only the owner
    // needs a sync. Syncing the item too would be harmless but is not what the C# sends.
    assert_eq!(effects, vec![Effect::SlotsChanged { owner: player }]);
}

#[test]
fn a_move_with_a_zero_count_is_still_a_whole_stack_move() {
    // The client sends 0 for "as much as fits"; the C# treats it as the whole stack.
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 10).unwrap();

    inventories.transfer_to_slot(player, 9, player, 12, 0);

    assert_eq!(inventories.slot(player, 12), Some(item));
    assert_eq!(inventories.count(item), Some(10));
}

// --- splitting -------------------------------------------------------------------------

#[test]
fn a_partial_count_onto_an_empty_slot_splits_the_stack() {
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 50).unwrap();

    let effects = inventories.transfer_to_slot(player, 9, player, 12, 25);

    assert_eq!(inventories.count(item), Some(25), "what was left behind");

    let created = inventories.slot(player, 12).unwrap();

    assert_ne!(created, 0, "the split half landed somewhere");
    assert_ne!(created, item, "a split makes a second entity");
    assert_eq!(inventories.count(created), Some(25));
    assert_eq!(inventories.name(created), Some(dirt()));

    // The new entity must be announced *before* a slot points at it, or the slot references
    // an entity the client has never heard of.
    assert_eq!(
        effects,
        vec![
            Effect::ItemChanged { entity: item },
            Effect::ItemCreated { entity: created },
            Effect::SlotsChanged { owner: player },
        ],
    );
}

#[test]
fn a_split_of_the_whole_stack_is_a_move_not_a_split() {
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 50).unwrap();

    inventories.transfer_to_slot(player, 9, player, 12, 50);

    assert_eq!(inventories.slot(player, 9), Some(0));
    assert_eq!(inventories.slot(player, 12), Some(item), "the same entity");
}

// --- merging ---------------------------------------------------------------------------

#[test]
fn dropping_a_stack_on_a_matching_one_tops_it_up() {
    let (mut inventories, player) = player();

    let source = inventories.give(player, 9, dirt(), 10).unwrap();
    let target = inventories.give(player, 12, dirt(), 5).unwrap();

    let effects = inventories.swap(player, 9, player, 12);

    assert_eq!(inventories.count(target), Some(15));
    assert_eq!(inventories.slot(player, 9), Some(0), "the source emptied");

    // The drained source entity is destroyed, not left as a zero stack.
    assert_eq!(inventories.count(source), None);

    assert!(effects.contains(&Effect::ItemRemoved { entity: source }));
    assert!(effects.contains(&Effect::ItemChanged { entity: target }));
}

#[test]
fn a_merge_stops_at_the_stack_limit_and_leaves_the_rest_behind() {
    let (mut inventories, player) = player();

    let source = inventories.give(player, 9, dirt(), 30).unwrap();
    let target = inventories.give(player, 12, dirt(), 50).unwrap();

    inventories.swap(player, 9, player, 12);

    // The default limit is 64, so 14 fit and 16 stay put.
    assert_eq!(inventories.count(target), Some(64));
    assert_eq!(inventories.count(source), Some(16));
    assert_eq!(inventories.slot(player, 9), Some(source), "still there");
}

#[test]
fn a_full_target_is_not_a_merge_but_a_swap() {
    let (mut inventories, player) = player();

    let source = inventories.give(player, 9, dirt(), 10).unwrap();
    let target = inventories.give(player, 12, dirt(), 64).unwrap();

    inventories.swap(player, 9, player, 12);

    assert_eq!(inventories.slot(player, 9), Some(target));
    assert_eq!(inventories.slot(player, 12), Some(source));
}

#[test]
fn different_items_exchange_places_rather_than_merging() {
    let (mut inventories, player) = player();

    let source = inventories.give(player, 9, dirt(), 10).unwrap();
    let target = inventories.give(player, 12, stone(), 5).unwrap();

    let effects = inventories.swap(player, 9, player, 12);

    assert_eq!(inventories.slot(player, 9), Some(target));
    assert_eq!(inventories.slot(player, 12), Some(source));

    assert_eq!(inventories.count(source), Some(10), "counts are untouched");
    assert_eq!(inventories.count(target), Some(5));

    assert_eq!(effects, vec![Effect::SlotsChanged { owner: player }]);
}

#[test]
fn a_stack_limit_override_is_respected() {
    // Arrows stack to 999 in the game's own data; the model must not hardcode 64.
    let mut limits = StackLimits::default();

    limits.set(stone(), 999);

    let mut inventories = Inventories::new(limits, 1000);

    inventories.open(10, 45);

    let source = inventories.give(10, 9, stone(), 100).unwrap();
    let target = inventories.give(10, 12, stone(), 100).unwrap();

    inventories.swap(10, 9, 10, 12);

    assert_eq!(inventories.count(target), Some(200));
    assert_eq!(inventories.count(source), None, "all of it fitted");
}

// --- destroying ------------------------------------------------------------------------

#[test]
fn destroying_part_of_a_stack_shrinks_it() {
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 10).unwrap();

    let effects = inventories.destroy(player, 9, 4);

    assert_eq!(inventories.count(item), Some(6));
    assert_eq!(inventories.slot(player, 9), Some(item));

    assert_eq!(effects, vec![Effect::ItemChanged { entity: item }]);
}

#[test]
fn destroying_a_whole_stack_empties_the_square() {
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 10).unwrap();

    let effects = inventories.destroy(player, 9, 10);

    assert_eq!(inventories.slot(player, 9), Some(0));
    assert_eq!(inventories.count(item), None);

    assert_eq!(
        effects,
        vec![
            Effect::SlotsChanged { owner: player },
            Effect::ItemRemoved { entity: item },
        ],
    );
}

#[test]
fn destroying_with_a_zero_count_destroys_all_of_it() {
    // The client sends the whole stack for a plain delete, but the C# tolerates a zero and
    // treats it as "all of it". A port that read zero as "destroy nothing" would leave the
    // trash can looking broken.
    let (mut inventories, player) = player();

    inventories.give(player, 9, dirt(), 10).unwrap();

    inventories.destroy(player, 9, 0);

    assert_eq!(inventories.slot(player, 9), Some(0));
}

// --- across two inventories ------------------------------------------------------------

#[test]
fn a_stack_moves_from_a_chest_into_the_rucksack() {
    let (mut inventories, player) = player();

    let chest = 20;
    inventories.open(chest, 12);

    let item = inventories.give(chest, 0, dirt(), 10).unwrap();

    let effects = inventories.transfer_to_slot(chest, 0, player, 9, 10);

    assert_eq!(inventories.slot(chest, 0), Some(0));
    assert_eq!(inventories.slot(player, 9), Some(item));

    // Both ends changed, so both need a sync -- the chest is an entity the client is drawing
    // too. Sending only the player's would leave the chest square still showing the item.
    assert_eq!(
        effects,
        vec![
            Effect::SlotsChanged { owner: chest },
            Effect::SlotsChanged { owner: player },
        ],
    );
}

#[test]
fn half_a_stack_can_be_dragged_straight_out_of_a_chest() {
    let (mut inventories, player) = player();

    let chest = 20;
    inventories.open(chest, 12);

    let item = inventories.give(chest, 0, dirt(), 50).unwrap();

    inventories.transfer_to_slot(chest, 0, player, 9, 20);

    assert_eq!(inventories.count(item), Some(30), "left in the chest");

    let taken = inventories.slot(player, 9).unwrap();

    assert_eq!(inventories.count(taken), Some(20));
}

#[test]
fn take_all_empties_the_chest_into_the_rucksack() {
    let (mut inventories, player) = player();

    let chest = 20;
    inventories.open(chest, 12);

    inventories.give(chest, 0, dirt(), 10).unwrap();
    inventories.give(chest, 1, stone(), 5).unwrap();
    inventories.give(chest, 5, dirt(), 3).unwrap();

    inventories.transfer_all(chest, player);

    assert!(
        (0..12).all(|slot| inventories.slot(chest, slot) == Some(0)),
        "the chest is empty",
    );

    // 10 + 3 Dirt merged into one stack, and the Stone is its own -- so two squares, not
    // three. Merging first is what the C# does and what a player expects.
    let occupied: Vec<u32> = (0..45)
        .filter_map(|slot| inventories.slot(player, slot))
        .filter(|item| *item != 0)
        .collect();

    assert_eq!(occupied.len(), 2, "two stacks: {occupied:?}");

    let total: u32 = occupied
        .iter()
        .filter(|item| inventories.name(**item) == Some(dirt()))
        .filter_map(|item| inventories.count(*item))
        .sum();

    assert_eq!(total, 13, "all the Dirt, in one stack");
}

#[test]
fn take_all_never_fills_an_equipment_square() {
    // Slots below 9 are what the player is wearing or holding. Taking loot into them would
    // silently equip it, which is not what the button means.
    let (mut inventories, player) = player();

    let chest = 20;
    inventories.open(chest, 12);

    inventories.give(chest, 0, dirt(), 10).unwrap();

    inventories.transfer_all(chest, player);

    assert!(
        (0..9).all(|slot| inventories.slot(player, slot) == Some(0)),
        "nothing was equipped",
    );

    assert_ne!(inventories.slot(player, 9), Some(0));
}

#[test]
fn take_all_stops_when_the_rucksack_is_full_and_leaves_the_rest() {
    let (mut inventories, player) = player();

    let chest = 20;
    inventories.open(chest, 40);

    // Fill every rucksack square with a different item so nothing can merge.
    for slot in 9..45 {
        inventories
            .give(player, slot, skysaga_core::name_hash(&format!("Filler{slot}")), 1)
            .unwrap();
    }

    inventories.give(chest, 0, dirt(), 10).unwrap();

    inventories.transfer_all(chest, player);

    assert_ne!(
        inventories.slot(chest, 0),
        Some(0),
        "with nowhere to put it, the item stays in the chest",
    );
}

// --- equipping -------------------------------------------------------------------------

#[test]
fn equipping_armour_moves_it_into_the_equipment_square() {
    let (mut inventories, player) = player();

    let helmet = inventories.give(player, 9, skysaga_core::name_hash("Helmet"), 1).unwrap();

    let effects = inventories.equip(player, 9, 2);

    assert_eq!(inventories.slot(player, 2), Some(helmet));
    assert_eq!(inventories.slot(player, 9), Some(0));

    assert_eq!(effects, vec![Effect::SlotsChanged { owner: player }]);
}

#[test]
fn equipping_over_worn_armour_puts_the_old_piece_back_in_the_bag() {
    // A swap rather than an overwrite: otherwise the piece already worn is destroyed.
    let (mut inventories, player) = player();

    let worn = inventories.give(player, 2, skysaga_core::name_hash("OldHelmet"), 1).unwrap();
    let new = inventories.give(player, 9, skysaga_core::name_hash("NewHelmet"), 1).unwrap();

    inventories.equip(player, 9, 2);

    assert_eq!(inventories.slot(player, 2), Some(new));
    assert_eq!(inventories.slot(player, 9), Some(worn));
}

#[test]
fn binding_to_a_hand_slot_leaves_the_inventory_completely_alone() {
    // Equip slots 0 and 1 are the hands, and the hotbar holds *resource hashes*, not entity
    // ids -- the stack stays in the rucksack, where placing blocks takes from it.
    //
    // Two wrong versions preceded this in the C#. Swapping made the stack vanish from the bag
    // when dragged to the hotbar; pointing the hand slot at the same entity was worse, because
    // the client sums every slot holding that entity and one 50 stack displayed as 150.
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 50).unwrap();

    let effects = inventories.equip(player, 9, 0);

    assert_eq!(inventories.slot(player, 9), Some(item), "still in the bag");
    assert_eq!(inventories.slot(player, 0), Some(0), "and not in the hand");

    assert!(effects.is_empty(), "nothing to sync: {effects:?}");
}

// --- refusals --------------------------------------------------------------------------

#[test]
fn an_out_of_range_slot_changes_nothing() {
    let (mut inventories, player) = player();

    inventories.give(player, 9, dirt(), 10).unwrap();

    assert!(inventories.transfer_to_slot(player, 9, player, 99, 10).is_empty());
    assert!(inventories.transfer_to_slot(player, 99, player, 9, 10).is_empty());
    assert!(inventories.swap(player, 9, player, 99).is_empty());
    assert!(inventories.destroy(player, 99, 1).is_empty());

    assert_ne!(inventories.slot(player, 9), Some(0), "the item is untouched");
}

#[test]
fn an_entity_with_no_inventory_changes_nothing() {
    // A hostile or confused client naming an entity that is not a container. The C# logs and
    // returns; nothing may panic on it, because these are bytes from an untrusted peer.
    let (mut inventories, player) = player();

    inventories.give(player, 9, dirt(), 10).unwrap();

    assert!(inventories.transfer_to_slot(player, 9, 999, 0, 10).is_empty());
    assert!(inventories.transfer_all(999, player).is_empty());

    assert_ne!(inventories.slot(player, 9), Some(0));
}

#[test]
fn moving_an_empty_slot_changes_nothing() {
    let (mut inventories, player) = player();

    assert!(inventories.transfer_to_slot(player, 9, player, 12, 1).is_empty());
    assert!(inventories.swap(player, 9, player, 12).is_empty());
    assert!(inventories.destroy(player, 9, 1).is_empty());
}

#[test]
fn a_slot_dropped_onto_itself_changes_nothing() {
    let (mut inventories, player) = player();

    let item = inventories.give(player, 9, dirt(), 10).unwrap();

    assert!(inventories.swap(player, 9, player, 9).is_empty());

    assert_eq!(inventories.slot(player, 9), Some(item));
    assert_eq!(inventories.count(item), Some(10), "it did not merge with itself");
}
