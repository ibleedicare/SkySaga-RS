//! Reading the client's `Data/data.pc`.
//!
//! The game's JSON — `Entities.json`, `geodata.json` — is not in this repository and cannot
//! be: it is the game's own data, and shipping it would be redistributing someone else's
//! copyrighted work. What *can* be shipped is the code that unpacks it, so a player who owns
//! the game can produce the files from their own copy. That is what this is.
//!
//! # The container
//!
//! Worked out from build 10414 and build 20015, which agree on every field. Little-endian
//! throughout.
//!
//! ```text
//! offset  size  what
//! 0x00       4  a per-file value, not a magic number: it differs between builds
//! 0x0c       4  entry count
//! 0x17c     48  entry 0, then one every 48 bytes
//! ```
//!
//! Each entry:
//!
//! ```text
//! +0x04      4  sector: the payload begins at sector * 16
//! +0x08      4  name hash, matched against the DICT below
//! +0x0c      4  unpacked size
//! +0x10      4  packed size -- equal to the unpacked size when the payload is stored raw
//! +0x18      4  tag: `BDAT` for a file, `DICT` for the name table
//! ```
//!
//! The `DICT` entry's payload is the name table, stored raw:
//!
//! ```text
//! +0x00     40  a header; `+0x0c` repeats this entry's own hash and `+0x10` its own size
//! +0x28      4  record count
//! +0x30     16  one record each: name offset, padding, name hash, padding
//! then          the names, NUL-separated, indexed by those offsets
//! ```
//!
//! So a name is found by looking its hash up in the table. The hash function itself is not
//! needed and is not implemented here — the archive carries both sides of the mapping.
//!
//! # What is not known
//!
//! `+0x00`, and the fields at `+0x00` and `+0x14` of each entry, are always zero in both
//! samples, and `+0x08` of the header (32) and `+0x10` (24) never vary. They are left
//! unread rather than guessed at.

use std::collections::HashMap;

/// Where the entry table starts.
const TABLE: usize = 0x17c;

/// One entry to the next.
const STRIDE: usize = 48;

/// Payload offsets are stored divided by this.
const SECTOR: usize = 16;

/// A file inside the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The name from the archive's own table, e.g. `entities.json`. `None` when the entry's
    /// hash is not in the table, which no observed archive does but the format allows.
    pub name: Option<String>,

    /// `BDAT` for a file, `DICT` for the name table.
    pub tag: [u8; 4],

    /// The payload, decompressed if it was compressed.
    pub data: Vec<u8>,
}

impl Member {
    /// The name to write this out as, falling back to the hash when the archive did not name
    /// it. Never a path: a member name with a separator in it would otherwise be able to
    /// write outside the output directory.
    pub fn file_name(&self, hash: u32) -> String {
        match &self.name {
            Some(name) if !name.is_empty() && !name.contains(['/', '\\']) => name.clone(),
            _ => format!("{hash:08x}.bin"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("truncated: the archive is {size} bytes but {what} needs {need}")]
    Truncated {
        what: &'static str,
        need: usize,
        size: usize,
    },

    #[error("implausible entry count {0}")]
    EntryCount(u32),

    #[error("decompressing {name}: {source}")]
    Inflate {
        name: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{name} unpacked to {got} bytes, but the entry says {want}")]
    Size {
        name: String,
        got: usize,
        want: usize,
    },
}

/// An unpacked `data.pc`.
#[derive(Debug, Clone)]
pub struct Archive {
    pub members: Vec<(u32, Member)>,
}

impl Archive {
    /// Parse an archive held in memory.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let count = read_u32(bytes, 0x0c, "the header")? as usize;

        // An archive of five members is what both samples hold. The bound is loose on
        // purpose -- it is here to turn a wrong offset into an error rather than a huge
        // allocation, not to encode a real limit.
        if count == 0 || count > 4096 {
            return Err(Error::EntryCount(count as u32));
        }

        // The name table has to be read first, because it names everything else.
        let mut names: HashMap<u32, String> = HashMap::new();

        for index in 0..count {
            let entry = Entry::read(bytes, index)?;

            if &entry.tag == b"DICT" {
                names = parse_dict(&entry.payload(bytes, "DICT")?);
            }
        }

        let mut members = Vec::with_capacity(count);

        for index in 0..count {
            let entry = Entry::read(bytes, index)?;
            let name = names.get(&entry.hash).cloned();
            let label = name
                .clone()
                .unwrap_or_else(|| format!("{:08x}", entry.hash));
            let raw = entry.payload(bytes, "a member")?;

            // Stored raw when the two sizes agree, zlib otherwise. Both occur: the JSON is
            // compressed, the 55-byte dataversion and the name table are not.
            let data = if entry.packed == entry.unpacked {
                raw
            } else {
                inflate(&raw, &label)?
            };

            if data.len() != entry.unpacked {
                return Err(Error::Size {
                    name: label,
                    got: data.len(),
                    want: entry.unpacked,
                });
            }

            members.push((
                entry.hash,
                Member {
                    name,
                    tag: entry.tag,
                    data,
                },
            ));
        }

        Ok(Self { members })
    }

    /// The members that are files, i.e. everything but the name table.
    pub fn files(&self) -> impl Iterator<Item = (u32, &Member)> {
        self.members
            .iter()
            .filter(|(_, member)| &member.tag == b"BDAT")
            .map(|(hash, member)| (*hash, member))
    }
}

struct Entry {
    hash: u32,
    tag: [u8; 4],
    start: usize,
    packed: usize,
    unpacked: usize,
}

impl Entry {
    fn read(bytes: &[u8], index: usize) -> Result<Self, Error> {
        let at = TABLE + index * STRIDE;

        Ok(Self {
            start: read_u32(bytes, at + 0x04, "an entry")? as usize * SECTOR,
            hash: read_u32(bytes, at + 0x08, "an entry")?,
            unpacked: read_u32(bytes, at + 0x0c, "an entry")? as usize,
            packed: read_u32(bytes, at + 0x10, "an entry")? as usize,
            tag: {
                let end = at + 0x1c;

                bytes
                    .get(at + 0x18..end)
                    .ok_or(Error::Truncated {
                        what: "an entry",
                        need: end,
                        size: bytes.len(),
                    })?
                    .try_into()
                    .expect("a four byte slice")
            },
        })
    }

    fn payload(&self, bytes: &[u8], what: &'static str) -> Result<Vec<u8>, Error> {
        let end = self.start + self.packed;

        bytes
            .get(self.start..end)
            .map(<[u8]>::to_vec)
            .ok_or(Error::Truncated {
                what,
                need: end,
                size: bytes.len(),
            })
    }
}

/// Where the record count sits inside the `DICT` payload, and where the records follow it.
const DICT_COUNT: usize = 0x28;
const DICT_RECORDS: usize = 0x30;
const DICT_RECORD: usize = 16;

fn parse_dict(dict: &[u8]) -> HashMap<u32, String> {
    let Ok(count) = read_u32(dict, DICT_COUNT, "the name table") else {
        return HashMap::new();
    };

    // The names begin where the records end. Bounded so a wrong count cannot make this loop
    // for a long time over a short buffer.
    let count = (count as usize).min(dict.len() / DICT_RECORD);
    let names_at = DICT_RECORDS + count * DICT_RECORD;
    let mut names = HashMap::new();

    for index in 0..count {
        let at = DICT_RECORDS + index * DICT_RECORD;

        let (Ok(offset), Ok(hash)) = (
            read_u32(dict, at, "a name record"),
            read_u32(dict, at + 8, "a name record"),
        ) else {
            continue;
        };

        let from = names_at + offset as usize;

        // Names are NUL-separated, and the last one is NUL-terminated too, so a missing
        // terminator means the table is malformed rather than that the name runs to the end.
        if let Some(rest) = dict.get(from..) {
            if let Some(end) = rest.iter().position(|byte| *byte == 0) {
                if let Ok(name) = std::str::from_utf8(&rest[..end]) {
                    names.insert(hash, name.to_owned());
                }
            }
        }
    }

    names
}

fn inflate(bytes: &[u8], name: &str) -> Result<Vec<u8>, Error> {
    use std::io::Read;

    let mut out = Vec::new();

    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut out)
        .map_err(|source| Error::Inflate {
            name: name.to_owned(),
            source,
        })?;

    Ok(out)
}

fn read_u32(bytes: &[u8], at: usize, what: &'static str) -> Result<u32, Error> {
    let end = at + 4;

    let slice = bytes.get(at..end).ok_or(Error::Truncated {
        what,
        need: end,
        size: bytes.len(),
    })?;

    Ok(u32::from_le_bytes(
        slice.try_into().expect("a four byte slice"),
    ))
}

/// Rewrite CRLF as LF.
///
/// The archive stores the JSON with Windows line endings. Normalising makes the output
/// byte-identical to the copies in the C# emulator's tree, which is what makes "did this
/// extract correctly" a question with a checkable answer.
pub fn to_unix_line_endings(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;

            continue;
        }

        out.push(bytes[index]);
        index += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an archive the way the game does, so the parser can be tested without the
    /// game's data. Every offset here is the format as documented at the top of this file;
    /// if the two disagree, one of them is wrong.
    fn synthetic(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let count = files.len() + 1;
        let mut out = vec![0u8; TABLE + count * STRIDE];

        out[0x0c..0x10].copy_from_slice(&(count as u32).to_le_bytes());

        // The name table: a 40-byte header, the count, the records, then the NUL-separated
        // names the records index into.
        let mut records = Vec::new();
        let mut names = Vec::new();

        for (index, (name, _, _)) in files.iter().enumerate() {
            records.extend_from_slice(&(names.len() as u32).to_le_bytes());
            records.extend_from_slice(&0u32.to_le_bytes());
            records.extend_from_slice(&hash(index).to_le_bytes());
            records.extend_from_slice(&0u32.to_le_bytes());

            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }

        let mut dict = vec![0u8; DICT_COUNT];

        dict.extend_from_slice(&(files.len() as u32).to_le_bytes());
        dict.resize(DICT_RECORDS, 0);
        dict.extend_from_slice(&records);
        dict.extend_from_slice(&names);

        let mut payloads: Vec<(u32, [u8; 4], Vec<u8>, usize)> = Vec::new();

        for (index, (_, body, compress)) in files.iter().enumerate() {
            let stored = if *compress {
                deflate(body)
            } else {
                body.to_vec()
            };

            payloads.push((hash(index), *b"BDAT", stored, body.len()));
        }

        payloads.push((u32::MAX, *b"DICT", dict.clone(), dict.len()));

        for (index, (hash, tag, stored, unpacked)) in payloads.iter().enumerate() {
            // Pad to a sector boundary, since offsets are stored divided by 16.
            while out.len() % SECTOR != 0 {
                out.push(0);
            }

            let at = TABLE + index * STRIDE;
            let sector = (out.len() / SECTOR) as u32;

            out[at + 0x04..at + 0x08].copy_from_slice(&sector.to_le_bytes());
            out[at + 0x08..at + 0x0c].copy_from_slice(&hash.to_le_bytes());
            out[at + 0x0c..at + 0x10].copy_from_slice(&(*unpacked as u32).to_le_bytes());
            out[at + 0x10..at + 0x14].copy_from_slice(&(stored.len() as u32).to_le_bytes());
            out[at + 0x18..at + 0x1c].copy_from_slice(tag);

            out.extend_from_slice(stored);
        }

        out
    }

    fn hash(index: usize) -> u32 {
        0x1000_0000 + index as u32
    }

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;

        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());

        encoder.write_all(bytes).expect("compressing");
        encoder.finish().expect("compressing")
    }

    #[test]
    fn reads_a_compressed_member_and_names_it() {
        let body = b"{\r\n    \"Entities\": []\r\n}".repeat(64);
        let archive =
            Archive::parse(&synthetic(&[("entities.json", &body, true)])).expect("parses");

        let files: Vec<_> = archive.files().collect();

        assert_eq!(files.len(), 1, "the DICT is not a file");
        assert_eq!(files[0].1.name.as_deref(), Some("entities.json"));
        assert_eq!(files[0].1.data, body);
    }

    /// The small members -- `dataversion.json` and the name table -- are stored with no
    /// compression, which the parser has to notice from the two sizes being equal rather
    /// than by trying to inflate and failing.
    #[test]
    fn reads_an_uncompressed_member() {
        let body = b"{\r\n    \"version\": \"abc\"\r\n}";
        let archive =
            Archive::parse(&synthetic(&[("dataversion.json", body, false)])).expect("parses");

        assert_eq!(archive.files().next().expect("a file").1.data, body);
    }

    #[test]
    fn reads_several_members_of_both_kinds() {
        let big = b"{\r\n\"AIAwarenessTraits\": []\r\n}".repeat(100);

        let archive = Archive::parse(&synthetic(&[
            ("entities.json", b"{\"Entities\":[]}", false),
            ("geodata.json", &big, true),
            ("dataversion.json", b"{}", false),
        ]))
        .expect("parses");

        let names: Vec<_> = archive
            .files()
            .map(|(_, member)| member.name.clone().unwrap_or_default())
            .collect();

        assert_eq!(names, ["entities.json", "geodata.json", "dataversion.json"]);
    }

    /// A truncated archive must be an error, not a panic: this reads a file the tool did not
    /// produce and has no reason to trust.
    #[test]
    fn truncation_is_an_error() {
        let full = synthetic(&[("entities.json", &b"x".repeat(4096), true)]);

        for cut in [0, 8, TABLE, TABLE + 8, full.len() - 16] {
            assert!(
                Archive::parse(&full[..cut]).is_err(),
                "a {cut} byte archive parsed",
            );
        }
    }

    #[test]
    fn a_silly_entry_count_is_an_error() {
        let mut bytes = synthetic(&[("entities.json", b"{}", false)]);

        bytes[0x0c..0x10].copy_from_slice(&u32::MAX.to_le_bytes());

        assert!(matches!(Archive::parse(&bytes), Err(Error::EntryCount(_))));
    }

    /// A member whose name contains a path separator must not be able to write outside the
    /// output directory.
    #[test]
    fn a_member_name_is_never_a_path() {
        for name in ["../escape.json", "sub/dir.json", "back\\slash.json", ""] {
            let member = Member {
                name: Some(name.to_owned()),
                tag: *b"BDAT",
                data: Vec::new(),
            };

            assert_eq!(
                member.file_name(0xdead_beef),
                "deadbeef.bin",
                "for {name:?}"
            );
        }
    }

    #[test]
    fn line_endings_are_normalised() {
        assert_eq!(to_unix_line_endings(b"a\r\nb\r\n"), b"a\nb\n");
        assert_eq!(to_unix_line_endings(b"no newlines"), b"no newlines");
        // A lone CR is data, not a line ending, and is left alone.
        assert_eq!(to_unix_line_endings(b"a\rb"), b"a\rb");
    }
}
