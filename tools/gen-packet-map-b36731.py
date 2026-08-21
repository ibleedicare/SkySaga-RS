#!/usr/bin/env python3
"""Generate the 10414 <-> 36731 packet-id table from documentations/packets-b36731.md.

The doc's table is `| 36731 id | name | 10414 id |`, recovered from the client's own packet-name
pointer array at 0x01409524. Generating the Rust from it keeps one source of truth: correcting
the doc and re-running this is the only way the table changes.

Names 36731 *renamed* show "-" in the 10414 column, because the doc matches by name and a
rename looks like a removal plus an addition. ALIASES puts those halves back together, and is
the one piece of judgement in here — a wrong alias is worse than a missing one, because the
client acts on the packet as whatever that id really is in its own enum.

    uv run tools/gen-packet-map-b36731.py > rust-server-alpha10/crates/skysaga-proto/src/client_build.rs
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = ROOT / "documentations" / "packets-b36731.md"
# 10414's ids are this enum's ordinals: it has no explicit values, so position is the id.
ENUM = ROOT / "server" / "Servers" / "SkySaga.Game" / "PacketId.cs"

# 10414 name -> 36731 name. From ClientBuild.cs, which reversed them the same way.
ALIASES = {
    "SetClientEntity": "SetClientLocalPlayerEntity",
    "CharcterCreationResponse": "CharacterCreationResponse",
    "RequestAITree": "DebugRequestAITree",
    "DebugForceSendTimedAdventureStartMail": "DebugForceSendTimedAdventureMail",
}

ROW = re.compile(r"^\|\s*(\d+)\s*\|\s*`([A-Za-z0-9_]+)`\s*\|\s*(\d+|-)")

rows = []
for line in DOC.read_text().splitlines():
    match = ROW.match(line)
    if match:
        new_id, name, old = match.groups()
        rows.append((int(new_id), name, None if old == "-" else int(old)))

if not rows:
    sys.exit(f"{DOC}: no table rows matched — has the format changed?")

by_name = {name: new_id for new_id, name, _ in rows}

# The C# enum, in declaration order. Nothing in it carries an explicit value, so the ordinal
# is the position — asserted below rather than assumed.
MEMBER = re.compile(r"^\s{4}([A-Za-z_][A-Za-z0-9_]*)\s*(,|$)")
old_ids = {}
for line in ENUM.read_text().splitlines():
    if "=" in line:
        sys.exit(f"{ENUM}: '{line.strip()}' has an explicit value; position is no longer the id")
    match = MEMBER.match(line)
    if match:
        old_ids.setdefault(match.group(1), len(old_ids))

if old_ids.get("SentErrorToClient") != 0 or old_ids.get("ClientConnected") != 1:
    sys.exit(f"{ENUM}: parsed {len(old_ids)} members but the first two are not 0 and 1")

pairs = {}  # 10414 ordinal -> (36731 ordinal, name)
for new_id, name, old in rows:
    ours = old_ids.get(name)

    if ours is None:
        continue

    # The doc's own third column, cross-checked against the enum it was derived from. A
    # disagreement means one of the two has drifted, and guessing which would be a coin flip.
    if old is not None and old != ours:
        sys.exit(f"{name}: doc says 10414 id {old}, PacketId.cs says {ours}")

    pairs.setdefault(ours, (new_id, name))

# Packets 36731 renamed: name matching sees a removal plus an addition, so the halves have to
# be rejoined by hand.
for ours_name, renamed in sorted(ALIASES.items()):
    if renamed not in by_name:
        sys.exit(f"alias {ours_name} -> {renamed}: no such 36731 packet")

    if ours_name not in old_ids:
        sys.exit(f"alias {ours_name} -> {renamed}: no such 10414 packet")

    old = old_ids[ours_name]

    if old in pairs:
        sys.exit(f"alias {ours_name} would overwrite 10414 id {old} -> {pairs[old]}")

    pairs[old] = (by_name[renamed], f"{ours_name} (36731: {renamed})")

print(
    f"// {len(pairs)} of {len(old_ids)} 10414 packets exist in 36731.",
    file=sys.stderr,
)

print('//! Packet-id translation between client builds. **Generated** by')
print('//! `tools/gen-packet-map-b36731.py` from `documentations/packets-b36731.md`; edit the')
print("//! doc and re-run rather than editing here.")
print("//!")
print("//! Build 36731 (Alpha V10, 2017) has 341 packets against 10414's 160, and of the 116")
print("//! names present in both, **not one kept its id**. There is no constant offset, so a")
print("//! 36731 client needs the whole table. Ids here are *ordinals* — the wire id adds")
print("//! [`crate::bitstream::ID_USER_PACKET_ENUM`].")
print()
print("/// Which client build a connection speaks.")
print("#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]")
print("pub enum ClientBuild {")
print("    /// Retail 2015. The ids every packet struct declares.")
print("    #[default]")
print("    B10414,")
print("    /// Alpha V10, 2017.")
print("    B36731,")
print("}")
print()
print("impl ClientBuild {")
print("    /// `SKYSAGA_CLIENT_BUILD=36731` selects Alpha V10; anything else is retail.")
print("    pub fn from_env() -> Self {")
print('        match std::env::var("SKYSAGA_CLIENT_BUILD").as_deref() {')
print('            Ok("36731") => Self::B36731,')
print("            _ => Self::B10414,")
print("        }")
print("    }")
print()
print("    /// Our ordinal -> this build's ordinal. `None` means the build has no such packet,")
print("    /// which must not be sent: a wrong id is worse than silence, because the client")
print("    /// would act on it as whatever that id means to it.")
print("    pub fn to_wire(self, ordinal: u16) -> Option<u16> {")
print("        match self {")
print("            Self::B10414 => Some(ordinal),")
print("            Self::B36731 => B36731_FROM_10414")
print("                .iter()")
print("                .find(|(ours, _, _)| *ours == ordinal)")
print("                .map(|(_, theirs, _)| *theirs),")
print("        }")
print("    }")
print()
print("    /// This build's ordinal -> ours.")
print("    pub fn from_wire(self, ordinal: u16) -> Option<u16> {")
print("        match self {")
print("            Self::B10414 => Some(ordinal),")
print("            Self::B36731 => B36731_FROM_10414")
print("                .iter()")
print("                .find(|(_, theirs, _)| *theirs == ordinal)")
print("                .map(|(ours, _, _)| *ours),")
print("        }")
print("    }")
print("}")
print()
print("/// `(10414 ordinal, 36731 ordinal, name)`, sorted by our ordinal.")
print("pub static B36731_FROM_10414: &[(u16, u16, &str)] = &[")
for old in sorted(pairs):
    new_id, name = pairs[old]
    print(f'    ({old}, {new_id}, "{name}"),')
print("];")
print()
print("#[cfg(test)]")
print("mod tests {")
print("    use super::*;")
print()
print("    #[test]")
print("    fn every_mapping_round_trips() {")
print("        for (ours, theirs, name) in B36731_FROM_10414 {")
print("            assert_eq!(")
print("                ClientBuild::B36731.to_wire(*ours),")
print("                Some(*theirs),")
print('                "{name}"')
print("            );")
print("            assert_eq!(")
print("                ClientBuild::B36731.from_wire(*theirs),")
print("                Some(*ours),")
print('                "{name}"')
print("            );")
print("        }")
print("    }")
print()
print("    #[test]")
print("    fn no_ordinal_is_claimed_twice() {")
print("        // A duplicate on either side would make the lookups disagree with each other,")
print("        // and `find` would silently pick whichever came first.")
print("        let mut ours: Vec<u16> = B36731_FROM_10414.iter().map(|(o, _, _)| *o).collect();")
print("        let mut theirs: Vec<u16> = B36731_FROM_10414.iter().map(|(_, t, _)| *t).collect();")
print("        let (before_ours, before_theirs) = (ours.len(), theirs.len());")
print()
print("        ours.sort_unstable();")
print("        ours.dedup();")
print("        theirs.sort_unstable();")
print("        theirs.dedup();")
print()
print("        assert_eq!(ours.len(), before_ours);")
print("        assert_eq!(theirs.len(), before_theirs);")
print("    }")
print()
print("    #[test]")
print("    fn the_handshake_ids_match_the_client() {")
print("        // Confirmed against live traffic and the receive sink, not just the doc:")
print("        // ClientConnected is the 2017 client's first packet (observed msgId 137 = 134 + 3),")
print("        // and ServerInfo's handler calls the receive sink with 0x49 = 73.")
print("        assert_eq!(ClientBuild::B36731.from_wire(3), Some(1)); // ClientConnected")
print("        assert_eq!(ClientBuild::B36731.to_wire(58), Some(73)); // ServerInfo")
print("        assert_eq!(ClientBuild::B36731.to_wire(6), Some(8)); // MapDefinition")
print("        assert_eq!(ClientBuild::B36731.to_wire(104), Some(158)); // SetClientEntity, aliased")
print("    }")
print()
print("    #[test]")
print("    fn retail_is_the_identity() {")
print("        for ordinal in 0..160u16 {")
print("            assert_eq!(ClientBuild::B10414.to_wire(ordinal), Some(ordinal));")
print("            assert_eq!(ClientBuild::B10414.from_wire(ordinal), Some(ordinal));")
print("        }")
print("    }")
print("}")
