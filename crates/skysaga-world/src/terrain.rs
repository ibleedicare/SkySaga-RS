//! Terrain generation.
//!
//! A port of `World/TerrainGenerator.cs`, verified against the chunks the C# server actually
//! sent — see `tests/terrain.rs`. The noise is plain value noise over an integer hash, so it
//! reproduces exactly as long as the arithmetic matches: 32-bit wrapping multiplies and an
//! *arithmetic* right shift, which is what C#'s `>>` on `int` does and Rust's `>>` on `i32`
//! does too.
//!
//! # Chunk layout
//!
//! `ChunkSize` cubed voxels, one byte each, preceded by a one-byte compression mode:
//!
//! ```text
//! [0]      mode: 0 = raw
//! [1..]    voxels, indexed 1 + y*32*32 + h1*32 + h2   (h1 = z, h2 = x)
//! ```
//!
//! A chunk with no solid voxel is not sent at all.

/// Block ids. The C# looks these up by name in GeoData with these as fallbacks.
pub mod blocks {
    pub const AIR: u8 = u8::MAX;
    pub const DIRT: u8 = 0;
    pub const STONE: u8 = 2;
    pub const SAND: u8 = 24;
    pub const COPPER_DEPOSIT: u8 = 25;
    pub const IRON_DEPOSIT: u8 = 26;
    pub const LEAD_DEPOSIT: u8 = 33;
    pub const GOLD_DEPOSIT: u8 = 42;
}

pub const CHUNK_SIZE: usize = 32;
pub const VOXELS_PER_CHUNK: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// The emulator's defaults.
pub const DEFAULT_SEED: i32 = 1337;
pub const DEFAULT_SIZE_CHUNKS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainGenerator {
    pub seed: i32,
    pub size_chunks: usize,
}

impl Default for TerrainGenerator {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            size_chunks: DEFAULT_SIZE_CHUNKS,
        }
    }
}

impl TerrainGenerator {
    /// Generate one chunk, or `None` when it is entirely air — those are not sent.
    pub fn chunk(&self, chunk_x: usize, chunk_y: usize, chunk_z: usize) -> Option<Vec<u8>> {
        let mut voxels = vec![blocks::AIR; VOXELS_PER_CHUNK + 1];

        voxels[0] = 0; // raw, uncompressed

        let mut solid = false;

        for h1 in 0..CHUNK_SIZE {
            for h2 in 0..CHUNK_SIZE {
                let world_h1 = chunk_z * CHUNK_SIZE + h1;
                let world_h2 = chunk_x * CHUNK_SIZE + h2;

                for y in 0..CHUNK_SIZE {
                    let world_y = chunk_y * CHUNK_SIZE + y;

                    let material = self.material_at(world_h2 as i32, world_y as i32, world_h1 as i32);

                    if material == blocks::AIR {
                        continue;
                    }

                    voxels[1 + y * CHUNK_SIZE * CHUNK_SIZE + h1 * CHUNK_SIZE + h2] = material;
                    solid = true;
                }
            }
        }

        solid.then_some(voxels)
    }

    /// Where a player should appear: the middle column, three voxels above the surface.
    pub fn spawn(&self) -> (i32, i32, i32) {
        let centre = (self.size_chunks * CHUNK_SIZE / 2) as i32;
        let (surface, _) = self.column(centre, centre);

        (centre, surface + 3, centre)
    }

    /// The material at a world voxel.
    ///
    /// Note the argument order into [`Self::column`]: chunk generation builds columns as
    /// `column(z, x)`, so passing `x, z` here would read a different column entirely. The C#
    /// carries the same warning.
    pub fn material_at(&self, x: i32, y: i32, z: i32) -> u8 {
        let (surface, floor) = self.column(z, x);

        self.material(x, y, z, surface, floor)
    }

    /// Surface height and island underside for one column.
    ///
    /// Islands are thicker under high ground, which rounds their underside — SkySaga's world
    /// is floating islands rather than a solid map.
    fn column(&self, h1: i32, h2: i32) -> (i32, i32) {
        let surface = 14 + (fractal(h1 as f32 * 0.045, h2 as f32 * 0.045, self.seed) * 7.0) as i32;

        let thickness = 5
            + (fractal(
                h1 as f32 * 0.03 + 100.0,
                h2 as f32 * 0.03 + 100.0,
                self.seed.wrapping_add(7),
            ) * 9.0) as i32
            + (surface - 14) / 2;

        (surface, (surface - thickness).max(0))
    }

    fn material(&self, x: i32, y: i32, z: i32, surface: i32, floor: i32) -> u8 {
        if y > surface || y < floor {
            return blocks::AIR;
        }

        if y > surface - 4 {
            return blocks::SAND;
        }

        if y >= floor + 3 {
            return blocks::DIRT;
        }

        self.ore(x, y, z).unwrap_or(blocks::STONE)
    }

    fn ore(&self, x: i32, y: i32, z: i32) -> Option<u8> {
        let roll = hash(
            x.wrapping_mul(31).wrapping_add(y.wrapping_mul(17)),
            z.wrapping_mul(13).wrapping_add(y.wrapping_mul(7)),
            self.seed.wrapping_add(99),
        );

        if roll > 0.05 {
            return None;
        }

        Some(if roll < 0.006 {
            blocks::GOLD_DEPOSIT
        } else if roll < 0.016 {
            blocks::LEAD_DEPOSIT
        } else if roll < 0.030 {
            blocks::COPPER_DEPOSIT
        } else {
            blocks::IRON_DEPOSIT
        })
    }
}

/// Four octaves of value noise, normalised.
fn fractal(x: f32, y: f32, seed: i32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut normal = 0.0;

    for octave in 0..4 {
        total += value(x * frequency, y * frequency, seed.wrapping_add(octave)) * amplitude;
        normal += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }

    total / normal
}

fn value(x: f32, y: f32, seed: i32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;

    let fx = smooth(x - x0 as f32);
    let fy = smooth(y - y0 as f32);

    let top = lerp(hash(x0, y0, seed), hash(x0 + 1, y0, seed), fx);
    let bottom = lerp(hash(x0, y0 + 1, seed), hash(x0 + 1, y0 + 1, seed), fx);

    lerp(top, bottom, fy)
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Integer hash to `[0, 1)`.
///
/// Wrapping 32-bit multiplies and an **arithmetic** right shift, matching C#'s `int`
/// semantics. A logical shift here would produce different terrain.
fn hash(x: i32, y: i32, seed: i32) -> f32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(seed.wrapping_mul(1_274_126_177));

    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);

    ((h ^ (h >> 16)) & 0x7fff_ffff) as f32 / 0x7fff_ffff as f32
}
