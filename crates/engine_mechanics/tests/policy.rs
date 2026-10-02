//! Participation resolution, persistence and the inert default.
//!
//! The acceptance for DROP 0005.0 is that a world which says nothing behaves
//! exactly as it did in DROP 0004, and that a more specific scope can turn a
//! broader scope's decision *off* again.

use engine_core::MaterialId;
use engine_mechanics::{
    MechanicalProfile, MechanicalRegistry, MechanicalSettings, Participation, ParticipationPolicy,
    ParticipationSet, PhysicalSystem, PolicyScope, settings_from_json, settings_to_json,
};

#[test]
fn a_silent_policy_participates_in_nothing() {
    let policy = ParticipationPolicy::new();
    assert!(policy.is_silent());
    for system in PhysicalSystem::ALL {
        let resolution = policy.resolve_detail(system);
        assert!(
            !resolution.participates,
            "{system:?} participated under a silent policy"
        );
        assert_eq!(resolution.decided_by, None);
    }
}

#[test]
fn most_specific_scope_wins() {
    // Every ordered pair of scopes: the more specific one must decide.
    let order = PolicyScope::MOST_SPECIFIC_FIRST;
    for (specific_index, specific) in order.iter().copied().enumerate() {
        for broader in order.iter().copied().skip(specific_index + 1) {
            let policy = ParticipationPolicy::new()
                .with_scope(
                    broader,
                    ParticipationSet::new().disabling(PhysicalSystem::StructuralCapacity),
                )
                .with_scope(
                    specific,
                    ParticipationSet::new().enabling(PhysicalSystem::StructuralCapacity),
                );
            let resolution = policy.resolve_detail(PhysicalSystem::StructuralCapacity);
            assert!(
                resolution.participates,
                "{specific:?} should have overridden {broader:?}"
            );
            assert_eq!(resolution.decided_by, Some(specific));
        }
    }
}

#[test]
fn a_specific_scope_can_disable_what_a_broader_one_enabled() {
    // The fantasy case: the world runs full mechanics, this object does not.
    let policy = ParticipationPolicy::new()
        .with_scope(
            PolicyScope::World,
            ParticipationSet::new()
                .enabling(PhysicalSystem::StructuralCapacity)
                .enabling(PhysicalSystem::MaterialMechanics),
        )
        .with_scope(
            PolicyScope::Object,
            ParticipationSet::new().disabling(PhysicalSystem::StructuralCapacity),
        );

    let capacity = policy.resolve_detail(PhysicalSystem::StructuralCapacity);
    assert!(!capacity.participates);
    assert_eq!(capacity.decided_by, Some(PolicyScope::Object));

    // The sibling system the object said nothing about still inherits.
    let mechanics = policy.resolve_detail(PhysicalSystem::MaterialMechanics);
    assert!(mechanics.participates);
    assert_eq!(mechanics.decided_by, Some(PolicyScope::World));
}

#[test]
fn gameplay_override_beats_every_other_scope() {
    // The magical bridge: gameplay asserts it is held up, and nothing below
    // gets to argue.
    let enable_everything = PhysicalSystem::ALL
        .into_iter()
        .fold(ParticipationSet::new(), |set, system| {
            set.with(system, Participation::Enable)
        });
    let policy = ParticipationPolicy::new()
        .with_scope(PolicyScope::Engine, enable_everything)
        .with_scope(PolicyScope::World, enable_everything)
        .with_scope(PolicyScope::Object, enable_everything)
        .with_scope(
            PolicyScope::Gameplay,
            ParticipationSet::new().disabling(PhysicalSystem::StructuralCapacity),
        );

    let resolution = policy.resolve_detail(PhysicalSystem::StructuralCapacity);
    assert!(!resolution.participates);
    assert_eq!(resolution.decided_by, Some(PolicyScope::Gameplay));
}

#[test]
fn systems_resolve_independently() {
    // Take weapon damage and shed fragments, but never participate in
    // continuous structural capacity.
    let policy = ParticipationPolicy::new().with_scope(
        PolicyScope::World,
        ParticipationSet::new()
            .enabling(PhysicalSystem::AppliedDamage)
            .enabling(PhysicalSystem::ConnectivityDetachment)
            .enabling(PhysicalSystem::FragmentPhysics)
            .enabling(PhysicalSystem::Gravity)
            .disabling(PhysicalSystem::StructuralCapacity),
    );

    assert!(policy.participates(PhysicalSystem::AppliedDamage));
    assert!(policy.participates(PhysicalSystem::ConnectivityDetachment));
    assert!(policy.participates(PhysicalSystem::FragmentPhysics));
    assert!(policy.participates(PhysicalSystem::Gravity));
    assert!(!policy.participates(PhysicalSystem::StructuralCapacity));
    // Never mentioned, so still off.
    assert!(!policy.participates(PhysicalSystem::MaterialMechanics));
}

#[test]
fn resolution_order_is_canonical() {
    let policy = ParticipationPolicy::new();
    let order: Vec<_> = policy.resolved().map(|(system, _)| system).collect();
    assert_eq!(order, PhysicalSystem::ALL.to_vec());
}

#[test]
fn setting_a_silent_scope_stores_nothing() {
    let mut policy = ParticipationPolicy::new();
    policy.set_scope(PolicyScope::World, ParticipationSet::INHERIT_ALL);
    assert!(policy.is_silent());

    policy.set_scope(
        PolicyScope::World,
        ParticipationSet::new().enabling(PhysicalSystem::Gravity),
    );
    assert!(!policy.is_silent());

    // Clearing it back to silence removes the scope again.
    policy.set_scope(PolicyScope::World, ParticipationSet::INHERIT_ALL);
    assert!(policy.is_silent());
}

#[test]
fn an_absent_profile_is_not_a_defaulted_one() {
    let mut registry = MechanicalRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.get(MaterialId(1)), None);

    registry.insert(MechanicalProfile::new(MaterialId(1)));
    assert_eq!(registry.len(), 1);
    assert!(registry.contains(MaterialId(1)));
    // Still nothing for a material nobody described.
    assert_eq!(registry.get(MaterialId(2)), None);
}

#[test]
fn registry_iterates_in_material_id_order() {
    let mut registry = MechanicalRegistry::new();
    for id in [7u32, 2, 9, 1] {
        registry.insert(MechanicalProfile::new(MaterialId(id)));
    }
    let ids: Vec<_> = registry.materials().map(|id| id.0).collect();
    assert_eq!(ids, vec![1, 2, 7, 9]);
}

#[test]
fn settings_round_trip_through_json() {
    let mut registry = MechanicalRegistry::new();
    registry.insert(MechanicalProfile::new(MaterialId(3)));
    registry.insert(MechanicalProfile::new(MaterialId(1)));

    let settings = MechanicalSettings {
        registry,
        policy: ParticipationPolicy::new()
            .with_scope(
                PolicyScope::World,
                ParticipationSet::new().enabling(PhysicalSystem::MaterialMechanics),
            )
            .with_scope(
                PolicyScope::Object,
                ParticipationSet::new().disabling(PhysicalSystem::MaterialMechanics),
            ),
    };

    let json = settings_to_json(&settings).expect("serialize");
    let parsed = settings_from_json(&json).expect("deserialize");
    assert_eq!(parsed, settings);
    // And the serialized form is stable.
    assert_eq!(settings_to_json(&parsed).expect("re-serialize"), json);
}

#[test]
fn inert_settings_are_recognisably_inert() {
    let settings = MechanicalSettings::inert();
    assert!(settings.is_inert());

    let json = settings_to_json(&settings).expect("serialize");
    let parsed = settings_from_json(&json).expect("deserialize");
    assert!(parsed.is_inert());
}

#[test]
fn unknown_systems_and_scopes_are_ignored_not_rejected() {
    // A world written by a later build that knows about systems and scopes this
    // build has never heard of must still load.
    let json = r#"{
        "registry": [],
        "policy": {
            "world": { "material_mechanics": "enable", "thermal_diffusion": "enable" },
            "galaxy": { "material_mechanics": "disable" }
        }
    }"#;

    let settings = settings_from_json(json).expect("forward-compatible load");
    assert!(
        settings
            .policy
            .participates(PhysicalSystem::MaterialMechanics)
    );
    // The unknown scope did not silently become the most specific one.
    assert_eq!(
        settings
            .policy
            .resolve_detail(PhysicalSystem::MaterialMechanics)
            .decided_by,
        Some(PolicyScope::World)
    );
}

#[test]
fn a_world_with_no_sidecar_is_exactly_a_drop_0004_world() {
    // Nothing persisted at all is the state every pre-DROP-0005 world is in.
    let settings = MechanicalSettings::default();
    assert!(settings.is_inert());
    assert!(settings.registry.is_empty());
    for system in PhysicalSystem::ALL {
        assert!(!settings.policy.participates(system));
    }
}
