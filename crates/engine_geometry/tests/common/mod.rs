//! Shared fixtures for the geometry tests.
//!
//! Each integration test binary compiles this module separately, so not every
//! helper is used by every binary.
#![allow(dead_code)]

use engine_core::{CellPos, MaterialId, MaterialRegistry, Rgb, VolumePos};
use engine_geometry::{ExactCompiler, GreedyCompiler, QuadSet, SurfaceCompiler};
use engine_world::World;

pub const STONE: MaterialId = MaterialId(1);
pub const DIRT: MaterialId = MaterialId(2);
pub const BRASS: MaterialId = MaterialId(3);

pub fn materials() -> MaterialRegistry {
    let mut registry = MaterialRegistry::new();
    registry.define(1, Rgb::new(128, 128, 128), "stone");
    registry.define(2, Rgb::new(120, 72, 40), "dirt");
    registry.define(3, Rgb::new(181, 166, 66), "brass");
    registry
}

pub fn world() -> World {
    World::with_materials(materials())
}

/// A solid volume-sized block at the given volume address.
pub fn solid_volume(world: &mut World, volume: VolumePos, material: MaterialId) {
    let origin = volume.origin();
    let max = CellPos::new(origin.x + 15, origin.y + 15, origin.z + 15);
    world.fill_box(origin, max, Some(material));
}

pub fn exact(world: &World, volume: VolumePos) -> QuadSet {
    ExactCompiler.compile(world, volume)
}

pub fn greedy(world: &World, volume: VolumePos) -> QuadSet {
    GreedyCompiler.compile(world, volume)
}

/// Deterministic pseudo-random source, so "random" test worlds are reproducible
/// and a failure can always be replayed. SplitMix64.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, bound: u32) -> u32 {
        (self.next_u64() % u64::from(bound)) as u32
    }
}
