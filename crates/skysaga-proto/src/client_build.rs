//! Packet-id translation between client builds. **Generated** by
//! `tools/gen-packet-map-b36731.py` from `documentations/packets-b36731.md`; edit the
//! doc and re-run rather than editing here.
//!
//! Build 36731 (Alpha V10, 2017) has 341 packets against 10414's 160, and of the 116
//! names present in both, **not one kept its id**. There is no constant offset, so a
//! 36731 client needs the whole table. Ids here are *ordinals* — the wire id adds
//! [`crate::bitstream::ID_USER_PACKET_ENUM`].

/// Which client build a connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClientBuild {
    /// Retail 2015. The ids every packet struct declares.
    #[default]
    B10414,
    /// Alpha V10, 2017.
    B36731,
}

impl ClientBuild {
    /// `SKYSAGA_CLIENT_BUILD=36731` selects Alpha V10; anything else is retail.
    pub fn from_env() -> Self {
        match std::env::var("SKYSAGA_CLIENT_BUILD").as_deref() {
            Ok("36731") => Self::B36731,
            _ => Self::B10414,
        }
    }

    /// Our ordinal -> this build's ordinal. `None` means the build has no such packet,
    /// which must not be sent: a wrong id is worse than silence, because the client
    /// would act on it as whatever that id means to it.
    pub fn to_wire(self, ordinal: u16) -> Option<u16> {
        match self {
            Self::B10414 => Some(ordinal),
            Self::B36731 => B36731_FROM_10414
                .iter()
                .find(|(ours, _, _)| *ours == ordinal)
                .map(|(_, theirs, _)| *theirs),
        }
    }

    /// This build's ordinal -> ours.
    pub fn from_wire(self, ordinal: u16) -> Option<u16> {
        match self {
            Self::B10414 => Some(ordinal),
            Self::B36731 => B36731_FROM_10414
                .iter()
                .find(|(_, theirs, _)| *theirs == ordinal)
                .map(|(ours, _, _)| *ours),
        }
    }
}

/// `(10414 ordinal, 36731 ordinal, name)`, sorted by our ordinal.
pub static B36731_FROM_10414: &[(u16, u16, &str)] = &[
    (0, 1, "SentErrorToClient"),
    (1, 3, "ClientConnected"),
    (2, 4, "ClientReadyToSync"),
    (3, 5, "ClientReadyToPlay"),
    (4, 6, "ClientInitialSyncFinished"),
    (5, 7, "ClientEntitiesSyncFinished"),
    (6, 8, "MapDefinition"),
    (7, 12, "BeginSync"),
    (8, 13, "ChunkSync"),
    (9, 14, "PartialChunkEditsSync"),
    (10, 137, "RequestWorldTeleport"),
    (11, 139, "RequestCharacterServerTeleport"),
    (12, 140, "TransferToServer"),
    (13, 15, "RequestEquipInventoryItem"),
    (14, 16, "RequestUnEquipInventoryItem"),
    (21, 34, "SetCurrentEmote"),
    (22, 35, "TeleportBegins"),
    (23, 36, "KillOccurred"),
    (24, 39, "IFellTooFar"),
    (25, 41, "PlayerJoined"),
    (26, 42, "PlayerLeft"),
    (27, 45, "DebugRequestBasicGearLoadout"),
    (28, 46, "DebugRequestFinishTutorial"),
    (29, 47, "DebugRequestPVPLoadout"),
    (30, 175, "DebugRequestAddItemToInventory"),
    (31, 178, "DebugEnableMechanic"),
    (34, 50, "EventEffect"),
    (35, 51, "SetPlayerState"),
    (36, 52, "SetPlayerMentalState"),
    (37, 53, "SetCharacterCustomisationData"),
    (39, 54, "QueueRecipeOnEntity"),
    (40, 55, "CollectCraftedItemInSlot"),
    (42, 56, "CraftingFailed"),
    (43, 58, "NewResourceEncountered"),
    (44, 59, "InventoryItemDrop"),
    (45, 60, "InventoryItemDestroy"),
    (48, 61, "AssignCollectionResourcePickup"),
    (49, 62, "FinalizeCollectionResourcePickup"),
    (50, 63, "InventoryItemTransfer"),
    (51, 64, "InventoryItemTransferToSlot"),
    (52, 65, "InventoryItemTransferAll"),
    (53, 69, "InventoryItemSwap"),
    (55, 70, "StorageItemSell"),
    (56, 71, "StorageItemBuy"),
    (57, 72, "TimeSync"),
    (58, 73, "ServerInfo"),
    (59, 74, "EntityEvent"),
    (64, 81, "ExecuteEntityAction"),
    (66, 82, "SendBroadcastMessage"),
    (67, 83, "CraftingNotification"),
    (68, 85, "GetUserToken"),
    (69, 86, "SetUserToken"),
    (70, 87, "ApplyImpulse"),
    (72, 88, "TemperaturePing"),
    (73, 89, "CoreTemperatureAdjustment"),
    (74, 90, "AllowFriendToEditHomeIsland"),
    (80, 96, "PvPPlayerRequestJoinTeam"),
    (81, 97, "PvPWaitingInfo"),
    (83, 132, "JoinPvPWorld"),
    (84, 133, "JoinHomeWorld"),
    (85, 0, "QuitGame"),
    (86, 141, "DebugSetTeleporterTarget"),
    (87, 142, "RequestRespawn"),
    (93, 143, "NewMailRecieved"),
    (94, 144, "MailRead"),
    (96, 145, "MailCheck"),
    (97, 146, "DeleteMail"),
    (98, 147, "TakeMailAttachment"),
    (99, 148, "RemoteMailSynced"),
    (100, 153, "EntityAdd"),
    (101, 154, "EntitySync"),
    (102, 155, "EntityMoved"),
    (103, 156, "EntityRemoved"),
    (104, 158, "SetClientEntity (36731: SetClientLocalPlayerEntity)"),
    (105, 159, "PlayerSpawned"),
    (106, 160, "SetLookAtDirection"),
    (108, 162, "SaveCharacterName"),
    (109, 163, "CharcterCreationResponse (36731: CharacterCreationResponse)"),
    (111, 181, "DebugRequestAddEntity"),
    (112, 185, "RequestAITree (36731: DebugRequestAITree)"),
    (113, 187, "ReturnAIInfo"),
    (114, 189, "DebugSetJobRank"),
    (115, 191, "DebugSetJobChallengeProgress"),
    (116, 192, "DebugActivateJobChallenge"),
    (117, 193, "DebugResetJobChallenge"),
    (118, 194, "DebugCompleteJobChallenge"),
    (119, 195, "DebugDeactivateJobChallenge"),
    (121, 196, "DebugForceSendTimedAdventureStartMail (36731: DebugForceSendTimedAdventureMail)"),
    (122, 200, "DebugSetEasyBuilding"),
    (123, 201, "DebugLearnRecipe"),
    (124, 202, "DebugForgetRecipe"),
    (125, 203, "DebugStartAdventure"),
    (126, 204, "DebugEndAdventure"),
    (127, 207, "EnableDebugRendering"),
    (128, 208, "AddDebugPrimitiveAABox"),
    (129, 209, "AddDebugPrimitiveLine"),
    (130, 210, "AddDebugPrimitiveSphere"),
    (131, 213, "AddDebugPrimitiveText"),
    (132, 214, "AddDebugPrimitiveTriangle"),
    (133, 217, "RemoveDebugPrimitive"),
    (134, 218, "ChangeDebugPrimitive"),
    (135, 219, "DebugSetTimeOfDay"),
    (136, 220, "ServerStats"),
    (137, 221, "ServerProfiles"),
    (138, 222, "RequestServerProfiles"),
    (139, 223, "DebugPopulateLoadout"),
    (140, 224, "DebugEquipmentChanged"),
    (143, 227, "DebugSetDurabilityHealthFraction"),
    (144, 260, "EquipmentMaterialChanged"),
    (145, 261, "TodoListTaskErase"),
    (146, 262, "TodoListTaskAdd"),
    (147, 263, "TodoListTaskRemove"),
    (150, 265, "NotifyPhotoCaptured"),
    (151, 266, "PhotoDelete"),
    (152, 267, "PhotoValidated"),
    (153, 276, "MoveItemToCraftingDropSlot"),
    (154, 277, "RemoveItemFromCraftingDropSlot"),
    (155, 278, "PerformCraftingDropSlotAction"),
    (158, 280, "PlayerDodged"),
    (159, 281, "EntityDodged"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mapping_round_trips() {
        for (ours, theirs, name) in B36731_FROM_10414 {
            assert_eq!(
                ClientBuild::B36731.to_wire(*ours),
                Some(*theirs),
                "{name}"
            );
            assert_eq!(
                ClientBuild::B36731.from_wire(*theirs),
                Some(*ours),
                "{name}"
            );
        }
    }

    #[test]
    fn no_ordinal_is_claimed_twice() {
        // A duplicate on either side would make the lookups disagree with each other,
        // and `find` would silently pick whichever came first.
        let mut ours: Vec<u16> = B36731_FROM_10414.iter().map(|(o, _, _)| *o).collect();
        let mut theirs: Vec<u16> = B36731_FROM_10414.iter().map(|(_, t, _)| *t).collect();
        let (before_ours, before_theirs) = (ours.len(), theirs.len());

        ours.sort_unstable();
        ours.dedup();
        theirs.sort_unstable();
        theirs.dedup();

        assert_eq!(ours.len(), before_ours);
        assert_eq!(theirs.len(), before_theirs);
    }

    #[test]
    fn the_handshake_ids_match_the_client() {
        // Confirmed against live traffic and the receive sink, not just the doc:
        // ClientConnected is the 2017 client's first packet (observed msgId 137 = 134 + 3),
        // and ServerInfo's handler calls the receive sink with 0x49 = 73.
        assert_eq!(ClientBuild::B36731.from_wire(3), Some(1)); // ClientConnected
        assert_eq!(ClientBuild::B36731.to_wire(58), Some(73)); // ServerInfo
        assert_eq!(ClientBuild::B36731.to_wire(6), Some(8)); // MapDefinition
        assert_eq!(ClientBuild::B36731.to_wire(104), Some(158)); // SetClientEntity, aliased
    }

    #[test]
    fn retail_is_the_identity() {
        for ordinal in 0..160u16 {
            assert_eq!(ClientBuild::B10414.to_wire(ordinal), Some(ordinal));
            assert_eq!(ClientBuild::B10414.from_wire(ordinal), Some(ordinal));
        }
    }
}
