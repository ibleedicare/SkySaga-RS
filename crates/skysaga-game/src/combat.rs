//! Checking a swing: how much it hurt, and whether the client could plausibly have landed it.
//!
//! # The client does the hit detection
//!
//! This was got wrong once and is worth stating plainly. `EquippedItemUsed` says what is being
//! swung; the client then sends `PerformEntityActions` naming **the entity it struck**. The
//! server does not sweep for a target, and an earlier version of this file that did -- on the
//! strength of a reversing note claiming no hit packet exists -- produced a sword that did
//! nothing in game while every one of its tests passed.
//!
//! That leaves the server two jobs: decide what a landed swing is *worth*, and decide whether
//! to believe it.
//!
//! # The numbers are the game's, not ours
//!
//! Damage is `EquippedActions[action].ActionEntity` resolved through `AttackActions`. The
//! range is that action's `EntityAreaOfEffect.RangeFactor` against the attacker's own `Reach`.
//! Both are read from `geodata.json`; the only invented number here is [`BODY_RADIUS`], and it
//! is invented because the data describes reach to a *surface* and the server knows only
//! origins.
//!
//! # Why distance and not the arc
//!
//! `AreaOfEffects` also gives an arc in degrees, and it is deliberately not checked. Testing
//! it needs the player's facing, and the yaw field's units are unproven -- the C# reads it as
//! a float and gets a denormal, so there is no oracle. An arc test would refuse real hits for
//! a reason the server cannot verify, and the client already knows which way it swung.
//!
//! # What is not modelled
//!
//! Stamina, blocking, parrying, dodge immunity, stagger, and the attacker's own weapon
//! contribution (`Resources[].Attack.AttackStrength`, and the material's stat template). The
//! formula that combines a weapon's strength with its action's was never recovered from the
//! client, so the action's strength is used alone rather than guessed at -- see
//! `documentations/combat-and-health.md`. Every one of those is a change to this file and to
//! nothing on the wire.

use skysaga_world::geodata::EquippedAction;

use crate::world::POSITION_SCALE;

/// How far past its origin a target can be hit, in voxels.
///
/// `Reaches` measures to a surface: `Medium` is 1.5 and a player is not expected to stand
/// inside a knight to hit one. The server knows only entity origins, so the target's own body
/// has to be added back. One voxel, because that is the cell a creature's voxel link occupies.
///
/// **This is the one number here that is not from a data file.**
pub const BODY_RADIUS: f32 = 1.0;

/// How far away a claimed hit is still believed, in voxels.
///
/// # Why this is not the attacker's reach
///
/// It was, and it was wrong. Deriving the bound from `Reach * RangeFactor + BODY_RADIUS` gives
/// 4.5 voxels for a sword, and a measured session refused **5 of 13 real hits** at that bound,
/// at distances from 4.8 to 10.4. A player experiences that as a sword that works two swings
/// in three, which is barely better than one that never works.
///
/// Both numbers going into the comparison are unreliable, and neither can be fixed here:
///
/// * the attacker's position is whatever `EntityMoved` last reported, which lags a moving
///   player by as far as they walk between updates;
/// * the target's is where the server put it, while the client has a physics body that falls
///   and settles after it is spawned;
/// * and the position scale itself is unconfirmed. `combat-and-health.md` reads these fields
///   as 1/64 of a voxel where [`POSITION_SCALE`] is 32, and every refused distance above is
///   an ordinary melee range under the other reading.
///
/// There is a fourth reason, and it is the one that made the refusals *grow* over a fight:
/// **the server's idea of where a creature stands is frozen at its spawn.** A creature has a
/// physics body, so the client's copy falls, settles and slides down terrain, and nothing
/// reports that back. Measured distances climbed steadily from 4.8 to 17.1 across one fight
/// with a stationary knight.
///
/// So this is a rail against a client claiming a kill across the island, and nothing finer:
/// one chunk. The client's own hit detection knows the swing's real arc, its active window and
/// where both bodies actually are; refereeing it from here with worse data is how honest hits
/// get lost.
pub const MAX_HIT_DISTANCE: f32 = 32.0;

/// Where an attack came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attacker {
    /// In the client's position units, as `EntityMoved` last reported.
    pub position: [u32; 3],

    /// From the attacker's own `PhysicalProperties`, in voxels.
    ///
    /// Not used to judge a hit -- see [`MAX_HIT_DISTANCE`] -- but read from the data file and
    /// carried here for anything that later wants it.
    pub reach: f32,
}

/// Whether a hit on `target` is close enough to believe.
pub fn in_range(attacker: &Attacker, target: [u32; 3]) -> bool {
    distance(attacker.position, target) <= MAX_HIT_DISTANCE
}

/// Horizontal distance in voxels.
///
/// Height is ignored: a creature on a step or a player mid-jump is still in front of the
/// swing, and nothing replicates entity heights to compare against.
pub fn distance(from: [u32; 3], target: [u32; 3]) -> f32 {
    let dx = (target[0] as f32 - from[0] as f32) / POSITION_SCALE as f32;
    let dz = (target[2] as f32 - from[2] as f32) / POSITION_SCALE as f32;

    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player's own reach, and the sword's basic swing.
    fn basic() -> EquippedAction {
        EquippedAction {
            name: "Basic_Diagonal".to_owned(),
            attack_strength: 7,
            knockback: 7.0,
            area_of_effect: "DiagonalSlash".to_owned(),
            arc_degrees: 110.0,
            range_factor: 1.0,
        }
    }

    fn player_at(position: [u32; 3]) -> Attacker {
        Attacker {
            position,
            reach: 1.5,
        }
    }

    const ORIGIN: [u32; 3] = [1000, 1000, 1000];

    fn voxels_away(count: u32) -> [u32; 3] {
        [ORIGIN[0], ORIGIN[1], ORIGIN[2] + count * POSITION_SCALE]
    }

    /// **Every distance a real fight produced is believed.**
    ///
    /// These are the measured refusals from the session that exposed the bug: a stationary
    /// knight, a player hitting it, and a bound of 4.5 that threw away five of thirteen hits.
    /// They are asserted individually because a regression here is invisible in game except as
    /// "the sword works sometimes".
    #[test]
    fn the_distances_a_real_fight_produced_are_all_believed() {
        let attacker = player_at(ORIGIN);

        for measured in [4.84, 7.10, 7.84, 8.92, 10.40, 13.08, 14.45, 17.09] {
            let target = [
                ORIGIN[0],
                ORIGIN[1],
                ORIGIN[2] + (measured * POSITION_SCALE as f32) as u32,
            ];

            assert!(in_range(&attacker, target), "{measured} voxels was a real hit");
        }
    }

    /// **Direction is not judged.** The client knows which way it swung; the yaw units do not.
    #[test]
    fn a_target_behind_the_attacker_is_still_in_range() {
        let attacker = player_at(ORIGIN);

        let behind = [ORIGIN[0], ORIGIN[1], ORIGIN[2] - 2 * POSITION_SCALE];

        assert!(in_range(&attacker, behind));
    }

    #[test]
    fn a_target_in_the_same_place_is_in_range() {
        assert!(in_range(&player_at(ORIGIN), ORIGIN));
    }

    /// Height is ignored, so a creature standing on a step is still in range.
    #[test]
    fn height_does_not_take_a_target_out_of_range() {
        let attacker = player_at(ORIGIN);

        let raised = [
            ORIGIN[0],
            ORIGIN[1] + 10 * POSITION_SCALE,
            ORIGIN[2] + 2 * POSITION_SCALE,
        ];

        assert!(in_range(&attacker, raised));
    }

    /// Across the island is refused, which is the only thing this check is for.
    #[test]
    fn a_target_across_the_island_is_refused() {
        assert!(!in_range(&player_at(ORIGIN), voxels_away(200)));
    }
}
