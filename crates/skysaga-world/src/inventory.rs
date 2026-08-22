//! What a drag, a split, a merge, a trash and a "take all" do to an inventory.
//!
//! Every inventory in the world lives here, and so does every item entity in one. That is the
//! game's own model rather than a convenience: a stack *is* an entity carrying an
//! [`InventoryItemComponent`], and an inventory is a list of those entities' ids. Splitting a
//! stack means creating a second entity; draining one means destroying it.
//!
//! # Why this is a crate with no I/O
//!
//! In the C# the same logic is spread across seven packet handlers and five helpers on a 2800
//! line `Connection`, all of which need a RakNet socket to reach. Nothing about "half of a
//! stack of 50 goes to an empty square" is networking, so none of it is here. The packet
//! handlers become two lines each: decode, then call one of these.
//!
//! # Effects, rather than sending
//!
//! An operation returns the [`Effect`]s it caused, in the order they must reach the client.
//! That ordering is not decoration: an [`Effect::ItemCreated`] **must** precede the
//! [`Effect::SlotsChanged`] that points a slot at the new entity, or the client is handed a
//! slot referencing an entity it has never been told about.

use std::collections::HashMap;

use skysaga_proto::types::InventorySlotData;

use crate::components::InventoryItemComponent;

/// The stack limit for an item that does not override it.
///
/// From the C#'s `DefaultStackLimit`.
pub const DEFAULT_STACK_LIMIT: u32 = 64;

/// The first slot of the rucksack proper. Anything below this is worn or held.
///
/// Duplicated from `skysaga-game`'s `FIRST_RUCKSACK_SLOT` because this crate must not depend
/// on that one; the two are asserted equal in `skysaga-game`'s tests.
pub const FIRST_RUCKSACK_SLOT: u32 = 9;

/// The equipment squares that are hands rather than storage.
///
/// Dropping something here binds it to the hotbar, which holds *resource hashes*, not entity
/// ids -- so the stack stays where it is. See [`Inventories::equip`].
const HAND_SLOTS: std::ops::Range<u32> = 0..2;

/// How large a stack of each item may be.
///
/// A map rather than a lookup into `geodata.json`: this crate reads no data files for it, so
/// the game server fills it from the resource table at startup and the tests fill it by hand.
#[derive(Debug, Clone, Default)]
pub struct StackLimits {
    overrides: HashMap<u32, u32>,
}

impl StackLimits {
    /// Override the limit for one item, by name hash.
    pub fn set(&mut self, name: u32, limit: u32) {
        self.overrides.insert(name, limit);
    }

    /// The limit for `name`, or [`DEFAULT_STACK_LIMIT`].
    pub fn get(&self, name: u32) -> u32 {
        self.overrides.get(&name).copied().unwrap_or(DEFAULT_STACK_LIMIT)
    }
}

/// Something the client has to be told about.
///
/// Returned in the order it must be sent. The game server turns each into a packet; nothing
/// here knows what a packet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// A new item entity exists. Becomes an `EntityAdd`, and must be sent before any
    /// `SlotsChanged` that references it.
    ItemCreated { entity: u32 },

    /// An item entity's stack changed size. Becomes an `EntitySync` of `inventoryslotdata`.
    ItemChanged { entity: u32 },

    /// An item entity is gone. Becomes an `EntityRemoved`.
    ItemRemoved { entity: u32 },

    /// An inventory's slot list changed. Becomes an `EntitySync` of `inventoryentitylist`.
    SlotsChanged { owner: u32 },
}

/// Every inventory in the world, and every item entity in one.
#[derive(Debug, Clone)]
pub struct Inventories {
    /// Owner entity id to its slots. A slot holds an item entity id, or 0 for empty.
    slots: HashMap<u32, Vec<u32>>,

    /// Item entity id to what that stack is.
    items: HashMap<u32, InventoryItemComponent>,

    limits: StackLimits,

    /// The next entity id to hand a newly split stack.
    ///
    /// Never reused. An id handed out twice would have the client destroy the entity it
    /// already holds under that id.
    next_entity_id: u32,
}

impl Inventories {
    /// An empty world, minting item entities from `next_entity_id` upwards.
    pub fn new(limits: StackLimits, next_entity_id: u32) -> Self {
        Self {
            slots: HashMap::new(),
            items: HashMap::new(),
            limits,
            next_entity_id,
        }
    }

    /// Use these stack limits from now on.
    ///
    /// Set once the game's own table has been read. The model is constructed before the data
    /// file is, so it starts with the default of 64 for everything and is corrected here
    /// rather than being unable to exist without the file.
    pub fn set_limits(&mut self, limits: StackLimits) {
        self.limits = limits;
    }

    /// Give `owner` an inventory of `size` empty slots. Replaces any it already had.
    pub fn open(&mut self, owner: u32, size: usize) {
        self.slots.insert(owner, vec![0; size]);
    }

    /// Forget an inventory, without destroying the items in it.
    ///
    /// For a player disconnecting: the body goes, and the items go with it because nothing
    /// else references them.
    pub fn close(&mut self, owner: u32) {
        if let Some(slots) = self.slots.remove(&owner) {
            for item in slots.into_iter().filter(|item| *item != 0) {
                self.items.remove(&item);
            }
        }
    }

    /// Whether `owner` has an inventory at all.
    pub fn is_open(&self, owner: u32) -> bool {
        self.slots.contains_key(&owner)
    }

    /// The whole slot list, for building the owner's `inventoryentitylist`.
    pub fn slots(&self, owner: u32) -> &[u32] {
        self.slots.get(&owner).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The item entity in one slot: `Some(0)` for an empty square, `None` for no such slot.
    pub fn slot(&self, owner: u32, slot: u32) -> Option<u32> {
        self.slots.get(&owner)?.get(slot as usize).copied()
    }

    /// The component describing an item entity, for serialising it.
    pub fn item(&self, entity: u32) -> Option<&InventoryItemComponent> {
        self.items.get(&entity)
    }

    /// How many are in a stack, or `None` if there is no such stack.
    pub fn count(&self, entity: u32) -> Option<u32> {
        Some(self.items.get(&entity)?.slot_data.count)
    }

    /// A stack's item, by name hash.
    pub fn name(&self, entity: u32) -> Option<u32> {
        self.items.get(&entity)?.slot_data.name
    }

    /// Create a stack of `name` and put it in `slot`, returning its entity id.
    ///
    /// `None` when there is no such slot or it is occupied. For seeding a world, for the
    /// admin `give`, and for the tests.
    pub fn give(&mut self, owner: u32, slot: u32, name: u32, count: u32) -> Option<u32> {
        if self.slot(owner, slot)? != 0 {
            return None;
        }

        let entity = self.create_stack(name, count);

        self.slots.get_mut(&owner)?[slot as usize] = entity;

        Some(entity)
    }

    /// The first free rucksack square, skipping what the player is wearing or holding.
    pub fn first_free_rucksack_slot(&self, owner: u32) -> Option<u32> {
        self.slots
            .get(&owner)?
            .iter()
            .enumerate()
            .skip(FIRST_RUCKSACK_SLOT as usize)
            .find(|(_, item)| **item == 0)
            .map(|(slot, _)| slot as u32)
    }

    // --- operations ---------------------------------------------------------------------

    /// A drop onto an **empty** square: move `count` of one slot to another.
    ///
    /// `count` below the source's stack size is a split; zero or the whole stack is a move.
    /// The two inventories may be the same one, which is a rearrange, or different, which is
    /// a chest transfer -- the client sends the identical packet for both.
    pub fn transfer_to_slot(
        &mut self,
        source_owner: u32,
        source_slot: u32,
        target_owner: u32,
        target_slot: u32,
        count: u32,
    ) -> Vec<Effect> {
        if !self.addressable(source_owner, source_slot, target_owner, target_slot) {
            return Vec::new();
        }

        let source_item = self.slot(source_owner, source_slot).unwrap_or(0);

        if source_item == 0 {
            return Vec::new();
        }

        // Onto an empty square, a partial count splits the stack in two.
        if let Some(effects) = self.try_split(
            source_owner,
            source_slot,
            target_owner,
            target_slot,
            count,
        ) {
            return effects;
        }

        // Onto a matching stack, top it up. The client normally sends `swap` for a drop onto
        // an occupied square, but it sends this one for a partial drag, so the merge has to
        // be reachable from both.
        if let Some(effects) = self.try_merge(
            source_owner,
            source_slot,
            target_owner,
            target_slot,
            count,
        ) {
            return effects;
        }

        self.exchange(source_owner, source_slot, target_owner, target_slot)
    }

    /// A drop onto an **occupied** square: merge the two stacks, or exchange them.
    pub fn swap(
        &mut self,
        source_owner: u32,
        source_slot: u32,
        target_owner: u32,
        target_slot: u32,
    ) -> Vec<Effect> {
        if !self.addressable(source_owner, source_slot, target_owner, target_slot) {
            return Vec::new();
        }

        // A square dropped onto itself. Not refused earlier because two *different* owners may
        // legitimately share a slot number.
        if source_owner == target_owner && source_slot == target_slot {
            return Vec::new();
        }

        if self.slot(source_owner, source_slot).unwrap_or(0) == 0 {
            return Vec::new();
        }

        // Count 0: as much of the stack as the target can hold.
        if let Some(effects) =
            self.try_merge(source_owner, source_slot, target_owner, target_slot, 0)
        {
            return effects;
        }

        self.exchange(source_owner, source_slot, target_owner, target_slot)
    }

    /// "Take All": every stack from one inventory into another.
    ///
    /// Tops up matching stacks before using empty squares, which is what the C# does and what
    /// a player expects -- ten dirt and three dirt come out as one stack of thirteen. Stops
    /// when the destination is full, leaving whatever did not fit where it was.
    pub fn transfer_all(&mut self, source_owner: u32, target_owner: u32) -> Vec<Effect> {
        if !self.is_open(source_owner) || !self.is_open(target_owner) {
            return Vec::new();
        }

        let mut effects = Vec::new();

        for source_slot in 0..self.slots(source_owner).len() as u32 {
            if self.slot(source_owner, source_slot).unwrap_or(0) == 0 {
                continue;
            }

            // Prefer topping up a matching stack. Written as a loop rather than a `find_map`
            // because the predicate and the body both need `self`, one shared and one unique.
            let mut merged = None;

            for target_slot in 0..self.slots(target_owner).len() as u32 {
                if self.slot(target_owner, target_slot).unwrap_or(0) == 0 {
                    continue;
                }

                merged =
                    self.try_merge(source_owner, source_slot, target_owner, target_slot, 0);

                if merged.is_some() {
                    break;
                }
            }

            if let Some(merged) = merged {
                effects.extend(merged);

                // A merge that hit the stack limit leaves a remainder behind; it still wants
                // a square of its own, so fall through rather than moving on.
                if self.slot(source_owner, source_slot).unwrap_or(0) == 0 {
                    continue;
                }
            }

            // Then the first free square -- never one of the equipment or hotbar squares,
            // which would silently equip the loot.
            let Some(free) = self.first_free_rucksack_slot(target_owner) else {
                break;
            };

            effects.extend(self.exchange(source_owner, source_slot, target_owner, free));
        }

        effects
    }

    /// The rucksack's trash can: destroy `count` from a slot. Zero means all of it.
    pub fn destroy(&mut self, owner: u32, slot: u32, count: u32) -> Vec<Effect> {
        let Some(item) = self.slot(owner, slot).filter(|item| *item != 0) else {
            return Vec::new();
        };

        let held = self.count(item).unwrap_or(0);

        // Zero is "all of it": the client sends the whole stack for a plain delete, but the C#
        // tolerates a zero and a port that read it as "destroy nothing" would look broken.
        if count > 0 && count < held {
            if let Some(stack) = self.items.get_mut(&item) {
                stack.slot_data.count = held - count;
            }

            return vec![Effect::ItemChanged { entity: item }];
        }

        self.slots.get_mut(&owner).expect("checked above")[slot as usize] = 0;
        self.items.remove(&item);

        vec![
            Effect::SlotsChanged { owner },
            Effect::ItemRemoved { entity: item },
        ]
    }

    /// Equip what is in `bag_slot` into `equip_slot`.
    ///
    /// Armour is a swap, so the piece already worn falls back into the square the new one came
    /// from rather than being destroyed.
    ///
    /// The **hands** are not a move at all. `hotbarslotresources` holds item name hashes, not
    /// entity ids, so a bound stack stays in the rucksack -- which is where placing blocks
    /// takes it from. Moving it produced two visible bugs in the C# before this was understood:
    /// swapping made the stack vanish from the bag, and pointing the hand slot at the same
    /// entity made the client sum both references and draw one stack of 50 as 150.
    pub fn equip(&mut self, owner: u32, bag_slot: u32, equip_slot: u32) -> Vec<Effect> {
        if !self.addressable(owner, bag_slot, owner, equip_slot) {
            return Vec::new();
        }

        if HAND_SLOTS.contains(&equip_slot) {
            return Vec::new();
        }

        if self.slot(owner, bag_slot).unwrap_or(0) == 0 {
            return Vec::new();
        }

        self.exchange(owner, bag_slot, owner, equip_slot)
    }

    // --- the primitives -----------------------------------------------------------------

    /// Whether both slots exist on inventories that exist.
    fn addressable(
        &self,
        source_owner: u32,
        source_slot: u32,
        target_owner: u32,
        target_slot: u32,
    ) -> bool {
        self.slot(source_owner, source_slot).is_some()
            && self.slot(target_owner, target_slot).is_some()
    }

    /// Exchange two slots' contents, and report whose lists changed.
    ///
    /// Both owners are reported when they differ: a chest the player is looking at is an
    /// entity the client draws, and syncing only the player leaves the chest square still
    /// showing an item that has moved.
    fn exchange(
        &mut self,
        source_owner: u32,
        source_slot: u32,
        target_owner: u32,
        target_slot: u32,
    ) -> Vec<Effect> {
        let source_item = self.slot(source_owner, source_slot).unwrap_or(0);
        let target_item = self.slot(target_owner, target_slot).unwrap_or(0);

        if let Some(slots) = self.slots.get_mut(&source_owner) {
            slots[source_slot as usize] = target_item;
        }

        if let Some(slots) = self.slots.get_mut(&target_owner) {
            slots[target_slot as usize] = source_item;
        }

        if source_owner == target_owner {
            vec![Effect::SlotsChanged {
                owner: source_owner,
            }]
        } else {
            vec![
                Effect::SlotsChanged {
                    owner: source_owner,
                },
                Effect::SlotsChanged {
                    owner: target_owner,
                },
            ]
        }
    }

    /// Split `count` off a stack into an **empty** square.
    ///
    /// `None` when this is not a split: an occupied target, or a count that is the whole
    /// stack, which is a plain move.
    fn try_split(
        &mut self,
        source_owner: u32,
        source_slot: u32,
        target_owner: u32,
        target_slot: u32,
        count: u32,
    ) -> Option<Vec<Effect>> {
        if self.slot(target_owner, target_slot)? != 0 {
            return None;
        }

        let source_item = self.slot(source_owner, source_slot)?;
        let stack = self.items.get(&source_item)?;

        if count == 0 || count >= stack.slot_data.count {
            return None;
        }

        let name = stack.slot_data.name;

        self.items.get_mut(&source_item)?.slot_data.count -= count;

        let created = self.create_stack(name.unwrap_or(0), count);

        // A stack whose name was absent stays absent, rather than becoming a hash of 0.
        if name.is_none() {
            self.items.get_mut(&created)?.slot_data.name = None;
        }

        self.slots.get_mut(&target_owner)?[target_slot as usize] = created;

        Some(vec![
            Effect::ItemChanged {
                entity: source_item,
            },
            // Before the slot list that points at it.
            Effect::ItemCreated { entity: created },
            Effect::SlotsChanged {
                owner: target_owner,
            },
        ])
    }

    /// Top up the target stack from the source, up to the stack limit.
    ///
    /// `None` when this is not a merge -- different items, an empty target, or no room --
    /// leaving the caller to exchange the two instead.
    fn try_merge(
        &mut self,
        source_owner: u32,
        source_slot: u32,
        target_owner: u32,
        target_slot: u32,
        count: u32,
    ) -> Option<Vec<Effect>> {
        if source_owner == target_owner && source_slot == target_slot {
            return None;
        }

        let source_item = self.slot(source_owner, source_slot)?;
        let target_item = self.slot(target_owner, target_slot)?;

        if source_item == 0 || target_item == 0 {
            return None;
        }

        let name = self.items.get(&source_item)?.slot_data.name?;

        if self.items.get(&target_item)?.slot_data.name != Some(name) {
            return None;
        }

        let held = self.items.get(&target_item)?.slot_data.count;
        let space = self.limits.get(name).saturating_sub(held);

        if space == 0 {
            return None;
        }

        let available = self.items.get(&source_item)?.slot_data.count;

        // A partial drag asks for that many; a whole-stack drag asks for all of it.
        let wanted = if count > 0 { count.min(available) } else { available };
        let moved = wanted.min(space);

        if moved == 0 {
            return None;
        }

        self.items.get_mut(&target_item)?.slot_data.count = held + moved;

        let left = available - moved;

        let mut effects = vec![Effect::ItemChanged {
            entity: target_item,
        }];

        if left > 0 {
            self.items.get_mut(&source_item)?.slot_data.count = left;

            effects.push(Effect::ItemChanged {
                entity: source_item,
            });
        } else {
            // Drained: the entity goes, rather than lingering as a stack of zero.
            self.slots.get_mut(&source_owner)?[source_slot as usize] = 0;
            self.items.remove(&source_item);

            effects.push(Effect::SlotsChanged {
                owner: source_owner,
            });
            effects.push(Effect::ItemRemoved {
                entity: source_item,
            });
        }

        Some(effects)
    }

    /// Mint an item entity.
    fn create_stack(&mut self, name: u32, count: u32) -> u32 {
        let entity = self.next_entity_id;
        self.next_entity_id += 1;

        self.items.insert(
            entity,
            InventoryItemComponent {
                slot_data: InventorySlotData {
                    name: Some(name),
                    count,
                    item_uuid: stack_uuid(entity),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        entity
    }

    /// The next entity id this will hand out, so the game server's allocator can stay ahead.
    pub fn next_entity_id(&self) -> u32 {
        self.next_entity_id
    }

    /// Mint from `next` upwards, if that is further along than where this already is.
    ///
    /// Never moves backwards: an id handed out twice would have the client destroy the entity
    /// it already holds under that id.
    pub fn reserve_ids_from(&mut self, next: u32) {
        self.next_entity_id = self.next_entity_id.max(next);
    }
}

/// A stack's own identity, derived from its entity id rather than drawn at random.
///
/// The client needs only to tell two piles of dirt apart, and an entity id is already unique
/// for the life of the server. Deriving it keeps this crate a function from values to values:
/// a random uuid would make every operation's output different on each run and put the whole
/// model beyond exact assertion.
fn stack_uuid(entity: u32) -> String {
    format!("00000000-0000-4000-8000-{entity:012x}")
}
