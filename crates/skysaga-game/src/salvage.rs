//! What a repair costs and what a dismantle returns, counted the way the client counts it.
//!
//! The client works both out itself and draws them before anything is sent: "2 Damaged" beside
//! a component, "Dismantling will return 5 components". The server has to arrive at the same
//! answer or the panel promises one thing and the rucksack shows another.
//!
//! # The count
//!
//! `FUN_008d7370` on the client's drop-slots component. With `total` the sum of the build
//! recipe's consumed quantities and `left` the item's durability over its maximum (1 for an
//! item that does not wear):
//!
//! - repair: `(total + 1) * (1 - left)`, at most `total`;
//! - dismantle: `(total + 1) * 0.5 * left`, at least 1.
//!
//! **In single precision, and truncated.** Measured on a pickaxe of 1500 whose recipe totals 9:
//! at 1200 the panel asks for 1 component where rounding gives 2, and at 300 it asks for 8
//! where truncating a double gives 7. Only `f32` arithmetic cut short fits both, and 5 at 700.
//!
//! # Which components
//!
//! One at a time, from a generator seeded with a number carried by the item itself
//! (`InventoryItemComponent +0x40`, the last numeric field of its slot data, which this server
//! sends as 0): `seed = seed * 0x343fd + 0x269ec3`, and bits 16 to 30 modulo the number of
//! ingredients still available pick one. An ingredient that runs out leaves the list.
//!
//! With seed 0 this gives 1 plate at 1200, 2 plates and 3 rods at 700, and 5 plates and 3 rods
//! at 300, which is what the panel drew for each (2026-10-11).

/// The one ingredient a dismantle always returns first, if the recipe has it.
pub const ALWAYS_RETURNED: &str = "Sharp_Rock";

/// How many components a repair replaces.
pub fn repair_count(total: u32, durability: u32, max: u32) -> u32 {
    let count = ((total + 1) as f32 * (1.0_f32 - left(durability, max))) as u32;

    count.min(total)
}

/// How many components a dismantle returns.
pub fn dismantle_count(total: u32, durability: u32, max: u32) -> u32 {
    let count = ((total + 1) as f32 * 0.5_f32 * left(durability, max)) as u32;

    count.max(1).min(total)
}

/// Durability over maximum, or 1 for an item that does not wear.
fn left(durability: u32, max: u32) -> f32 {
    if max == 0 {
        return 1.0;
    }

    durability.min(max) as f32 / max as f32
}

/// Draw `count` components out of `ingredients`, as `(index into ingredients, quantity)` in
/// the order each was first drawn.
///
/// `first` forces the first draw without turning the generator, which is how a dismantle
/// always returns its [`ALWAYS_RETURNED`] ingredient.
pub fn draw(seed: u32, ingredients: &[u32], count: u32, first: Option<usize>) -> Vec<(usize, u32)> {
    // What is still available, by original index.
    let mut available: Vec<(usize, u32)> = ingredients
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, quantity)| *quantity > 0)
        .collect();

    let mut drawn: Vec<(usize, u32)> = Vec::new();
    let mut seed = seed;

    for turn in 0..count {
        if available.is_empty() {
            break;
        }

        let forced = (turn == 0)
            .then_some(first)
            .flatten()
            .and_then(|index| available.iter().position(|(original, _)| *original == index));

        let position = forced.unwrap_or_else(|| {
            seed = seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);

            ((seed >> 16) & 0x7fff) as usize % available.len()
        });

        let original = available[position].0;

        match drawn.iter_mut().find(|(index, _)| *index == original) {
            Some((_, quantity)) => *quantity += 1,
            None => drawn.push((original, 1)),
        }

        available[position].1 -= 1;

        if available[position].1 == 0 {
            available.remove(position);
        }
    }

    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Metal_Pickaxe`: six plates and three rods, 1500 when new.
    const PICKAXE: [u32; 2] = [6, 3];

    fn repair(durability: u32) -> Vec<(usize, u32)> {
        draw(0, &PICKAXE, repair_count(9, durability, 1500), None)
    }

    /// The three splits the client's repair panel drew, plate first.
    #[test]
    fn a_repair_costs_what_the_panel_showed() {
        assert_eq!(repair(1200), vec![(0, 1)], "1 Damaged, 0 Damaged");
        assert_eq!(repair(700), vec![(0, 2), (1, 3)], "2 Damaged, 3 Damaged");
        assert_eq!(repair(300), vec![(0, 5), (1, 3)], "5 Damaged, 3 Damaged");
    }

    #[test]
    fn a_new_item_costs_nothing_and_a_spent_one_costs_everything() {
        assert_eq!(repair_count(9, 1500, 1500), 0);
        assert_eq!(repair_count(9, 0, 1500), 9);
        assert_eq!(repair(0), vec![(0, 6), (1, 3)]);
    }

    /// "Dismantling will return 5 components", for a new pickaxe.
    #[test]
    fn a_new_item_returns_half_of_one_more_than_it_took() {
        assert_eq!(dismantle_count(9, 1500, 1500), 5);
    }

    #[test]
    fn a_dismantle_always_returns_something() {
        assert_eq!(dismantle_count(9, 0, 1500), 1);
        assert_eq!(dismantle_count(1, 1, 1), 1);
    }

    #[test]
    fn something_that_does_not_wear_counts_as_new() {
        assert_eq!(dismantle_count(9, 0, 0), 5);
    }

    #[test]
    fn a_forced_first_draw_does_not_turn_the_generator() {
        // Seed 0 draws ingredient 0 first. Forcing ingredient 1 first, the draw after it is
        // still the generator's first, so ingredient 0 follows.
        assert_eq!(draw(0, &PICKAXE, 2, Some(1)), vec![(1, 1), (0, 1)]);
    }

    #[test]
    fn a_draw_never_takes_more_than_there_is() {
        let drawn = draw(7, &[1, 1], 5, None);

        assert_eq!(drawn.iter().map(|(_, quantity)| quantity).sum::<u32>(), 2);
    }
}
