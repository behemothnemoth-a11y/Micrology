//! Mechanical profiles: the inert data layer.
//!
//! Nothing here decides anything. These tests pin the two properties the rest
//! of the drop will lean on: an absent profile stays absent, and a profile
//! written by an older build loads without values being invented for it.

use engine_core::MaterialId;
use engine_mechanics::{
    FailureCharacter, LoadMode, MechanicalProfile, MechanicalRegistry, MechanicalSettings, Milli,
    ReferenceMaterial, reference_registry, settings_from_json, settings_to_json,
};

#[test]
fn capacity_selects_by_load_mode() {
    let profile = ReferenceMaterial::Masonry.profile(MaterialId(1));
    assert_eq!(profile.capacity(LoadMode::Tensile), profile.tensile);
    assert_eq!(profile.capacity(LoadMode::Compressive), profile.compressive);
    assert_eq!(profile.capacity(LoadMode::Shear), profile.shear);
}

#[test]
fn masonry_is_far_weaker_in_tension_than_compression() {
    // The relationship that makes an arch stand and a masonry beam fall. If a
    // later edit flattens the reference set, this is what catches it.
    let masonry = ReferenceMaterial::Masonry.profile(MaterialId(1));
    assert!(masonry.compressive.raw() > masonry.tensile.raw() * 10);

    // Steel does not share that asymmetry.
    let steel = ReferenceMaterial::Steel.profile(MaterialId(2));
    assert_eq!(steel.tensile, steel.compressive);
}

#[test]
fn glass_is_strong_but_not_tough() {
    // Glass out-carries concrete in tension and is far less able to take a hit.
    let glass = ReferenceMaterial::Glass.profile(MaterialId(1));
    let concrete = ReferenceMaterial::Concrete.profile(MaterialId(2));
    assert!(glass.tensile.raw() > concrete.tensile.raw());
    assert!(glass.toughness.raw() < concrete.toughness.raw());
    assert_eq!(glass.failure, FailureCharacter::Brittle);
}

#[test]
fn density_ordering_is_physically_recognisable() {
    let density = |m: ReferenceMaterial| m.profile(MaterialId(1)).density.raw();
    assert!(density(ReferenceMaterial::Wood) < density(ReferenceMaterial::Masonry));
    assert!(density(ReferenceMaterial::Masonry) < density(ReferenceMaterial::Concrete));
    assert!(density(ReferenceMaterial::Concrete) < density(ReferenceMaterial::Steel));
}

#[test]
fn an_inert_profile_exists_but_carries_nothing() {
    let profile = MechanicalProfile::inert(MaterialId(9));
    assert!(!profile.carries_load());
    assert_eq!(profile.density, Milli::ZERO);
    for mode in LoadMode::ALL {
        assert_eq!(profile.capacity(mode), Milli::ZERO);
    }
    // Existing-but-inert is not the same as absent.
    let mut registry = MechanicalRegistry::new();
    registry.insert(profile);
    assert_eq!(registry.get(MaterialId(9)), Some(profile));
    assert_eq!(registry.get(MaterialId(10)), None);
}

#[test]
fn every_reference_material_carries_load() {
    for material in ReferenceMaterial::ALL {
        let profile = material.profile(MaterialId(1));
        assert!(
            profile.carries_load(),
            "{} should carry load",
            material.name()
        );
        assert!(
            profile.density.is_positive(),
            "{} needs mass",
            material.name()
        );
    }
}

#[test]
fn reference_names_round_trip() {
    for material in ReferenceMaterial::ALL {
        assert_eq!(
            ReferenceMaterial::from_name(material.name()),
            Some(material)
        );
    }
    assert_eq!(ReferenceMaterial::from_name("unobtainium"), None);
}

#[test]
fn reference_registry_is_ordered_and_deterministic() {
    let assignments = [
        (ReferenceMaterial::Steel, MaterialId(40)),
        (ReferenceMaterial::Wood, MaterialId(10)),
        (ReferenceMaterial::Glass, MaterialId(50)),
        (ReferenceMaterial::Masonry, MaterialId(20)),
        (ReferenceMaterial::Concrete, MaterialId(30)),
    ];
    let first = reference_registry(assignments);
    let second = reference_registry(assignments);
    assert_eq!(first, second);

    // Insertion order does not leak into iteration order.
    let ids: Vec<_> = first.materials().map(|id| id.0).collect();
    assert_eq!(ids, vec![10, 20, 30, 40, 50]);
}

#[test]
fn a_caller_can_override_a_reference_profile() {
    let mut registry = reference_registry([(ReferenceMaterial::Wood, MaterialId(1))]);
    let mut custom = ReferenceMaterial::Wood.profile(MaterialId(1));
    custom.tensile = Milli::whole(999);
    registry.insert(custom);
    assert_eq!(registry.get(MaterialId(1)), Some(custom));
    assert_eq!(registry.len(), 1);
}

#[test]
fn profiles_round_trip_through_persistence() {
    let settings = MechanicalSettings {
        registry: reference_registry(
            ReferenceMaterial::ALL
                .into_iter()
                .enumerate()
                .map(|(index, material)| (material, MaterialId(index as u32 + 1))),
        ),
        policy: Default::default(),
    };

    let json = settings_to_json(&settings).expect("serialize");
    let parsed = settings_from_json(&json).expect("deserialize");
    assert_eq!(parsed, settings);
    assert_eq!(settings_to_json(&parsed).expect("re-serialize"), json);
}

#[test]
fn an_older_profile_loads_without_inventing_values() {
    // Exactly what DROP 0005.0 wrote: an id and nothing else.
    let json = r#"{ "registry": [ { "material": 7 } ], "policy": {} }"#;
    let settings = settings_from_json(json).expect("forward-compatible load");
    let profile = settings
        .registry
        .get(MaterialId(7))
        .expect("profile present");

    assert_eq!(profile, MechanicalProfile::inert(MaterialId(7)));
    assert!(!profile.carries_load());
    // Present but inert, never silently given reference values.
    assert_ne!(profile, ReferenceMaterial::Steel.profile(MaterialId(7)));
}

#[test]
fn an_unknown_profile_field_is_ignored_not_rejected() {
    let json = r#"{
        "registry": [ { "material": 3, "density": 2400, "thermal_expansion": 12 } ],
        "policy": {}
    }"#;
    let settings = settings_from_json(json).expect("forward-compatible load");
    let profile = settings.registry.get(MaterialId(3)).expect("profile");
    assert_eq!(profile.density, Milli(2_400));
}
