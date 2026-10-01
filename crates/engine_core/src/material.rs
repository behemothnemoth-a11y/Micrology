//! Engine-native materials.
//!
//! DROP 0001 deliberately keeps this minimal: a stable id, a base colour, and
//! an optional human-readable name. There is no texture atlas, no block-state
//! table and no PBR parameter set yet. New fields are additive — the save
//! format ignores unknown keys so that older files keep loading.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A stable, engine-native material identity.
///
/// Material ids are the engine's own; they are never Minecraft block-registry
/// ids. Import adapters are responsible for mapping foreign ids onto these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MaterialId(pub u32);

impl fmt::Display for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mat:{}", self.0)
    }
}

/// A 24-bit sRGB colour.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug, Serialize, Deserialize,
)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);
    /// Used wherever an unregistered material is encountered, so that data bugs
    /// are loud instead of invisible.
    pub const MISSING: Rgb = Rgb::new(255, 0, 255);

    #[inline]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Build from a packed `0xRRGGBB` value.
    #[inline]
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
        }
    }

    /// Pack into a `0xRRGGBB` value.
    #[inline]
    pub const fn to_hex(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    /// Components as sRGB floats in `0.0..=1.0`.
    ///
    /// This is the *encoded* value, suitable for UI and for writing back out.
    /// Renderers almost always want [`Rgb::to_linear_f32`] instead.
    #[inline]
    pub fn to_srgb_f32(self) -> [f32; 3] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        ]
    }

    /// Components as linear-light floats in `0.0..=1.0`.
    ///
    /// Shading maths is linear, and vertex colours are consumed as linear
    /// values. Handing a renderer raw sRGB components makes every surface too
    /// bright and washed out — mid-tones suffer worst, because that is where
    /// the sRGB transfer curve bends furthest from linear.
    #[inline]
    pub fn to_linear_f32(self) -> [f32; 3] {
        let channel = |value: u8| {
            let encoded = value as f32 / 255.0;
            if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        };
        [channel(self.r), channel(self.g), channel(self.b)]
    }
}

/// A material definition.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Material {
    pub id: MaterialId,
    /// Base colour. The only shading input DROP 0001 supports.
    pub color: Rgb,
    /// Optional human-readable name, for editors and debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Material {
    /// A material with a colour and no name.
    pub fn new(id: MaterialId, color: Rgb) -> Self {
        Self {
            id,
            color,
            name: None,
        }
    }

    /// A material with a colour and a name.
    pub fn named(id: MaterialId, color: Rgb, name: impl Into<String>) -> Self {
        Self {
            id,
            color,
            name: Some(name.into()),
        }
    }
}

/// The set of materials a world refers to.
///
/// Stored sorted by id so that iteration and serialization are deterministic.
///
/// Deliberately not `Serialize`: the save format writes materials as a sorted
/// list rather than a map, so that keys never have to be stringified.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct MaterialRegistry {
    materials: BTreeMap<MaterialId, Material>,
}

impl MaterialRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a material, returning the previous definition.
    pub fn insert(&mut self, material: Material) -> Option<Material> {
        self.materials.insert(material.id, material)
    }

    /// Convenience constructor for a named material.
    pub fn define(&mut self, id: u32, color: Rgb, name: &str) -> MaterialId {
        let id = MaterialId(id);
        self.insert(Material::named(id, color, name));
        id
    }

    pub fn get(&self, id: MaterialId) -> Option<&Material> {
        self.materials.get(&id)
    }

    pub fn contains(&self, id: MaterialId) -> bool {
        self.materials.contains_key(&id)
    }

    /// The colour of `id`, or [`Rgb::MISSING`] if it is not registered.
    pub fn color_of(&self, id: MaterialId) -> Rgb {
        self.get(id).map(|m| m.color).unwrap_or(Rgb::MISSING)
    }

    /// Materials in ascending id order.
    pub fn iter(&self) -> impl Iterator<Item = &Material> {
        self.materials.values()
    }

    /// Material ids in ascending order.
    pub fn ids(&self) -> impl Iterator<Item = MaterialId> + '_ {
        self.materials.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }
}

impl FromIterator<Material> for MaterialRegistry {
    fn from_iter<T: IntoIterator<Item = Material>>(iter: T) -> Self {
        let mut registry = Self::new();
        for material in iter {
            registry.insert(material);
        }
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for hex in [0x000000u32, 0xffffff, 0x1e90ff, 0x7f3a01] {
            assert_eq!(Rgb::from_hex(hex).to_hex(), hex);
        }
        assert_eq!(Rgb::from_hex(0x1e90ff), Rgb::new(0x1e, 0x90, 0xff));
    }

    #[test]
    fn registry_iterates_in_id_order() {
        let mut reg = MaterialRegistry::new();
        reg.define(7, Rgb::new(1, 1, 1), "seven");
        reg.define(2, Rgb::new(2, 2, 2), "two");
        reg.define(9, Rgb::new(3, 3, 3), "nine");
        assert_eq!(
            reg.ids().collect::<Vec<_>>(),
            vec![MaterialId(2), MaterialId(7), MaterialId(9)]
        );
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn linear_conversion_darkens_mid_tones_and_pins_the_ends() {
        assert_eq!(Rgb::new(0, 0, 0).to_linear_f32(), [0.0, 0.0, 0.0]);
        assert_eq!(Rgb::WHITE.to_linear_f32(), [1.0, 1.0, 1.0]);

        // Mid grey is the case that exposes a missing conversion: sRGB 0.5
        // is only about 0.21 of the light.
        let mid = Rgb::new(128, 128, 128).to_linear_f32()[0];
        assert!(
            (mid - 0.2158).abs() < 0.001,
            "sRGB 128 should be ~0.216 linear, got {mid}"
        );
        for value in [1u8, 7, 10] {
            // The low end is the linear segment of the curve, not the power one.
            let expected = value as f32 / 255.0 / 12.92;
            assert!((Rgb::new(value, 0, 0).to_linear_f32()[0] - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn unregistered_material_reports_missing_colour() {
        let reg = MaterialRegistry::new();
        assert_eq!(reg.color_of(MaterialId(1)), Rgb::MISSING);
        assert!(!reg.contains(MaterialId(1)));
    }
}
