from pathlib import Path
p=Path(r"C:\Users\behem\.chatgpt-worktrees\Micrology-house-joint-load")/"crates/engine_stress/src/house_scenarios.rs"
s=p.read_text(encoding="utf-8")
def rep(old,new,count=1):
    global s
    assert s.count(old)==count,(old,s.count(old))
    s=s.replace(old,new,count)
rep("    reference_house, reference_house_joints, structural_state_digest,",
    "    reference_house, reference_house_joint_load, reference_house_joints, structural_state_digest,")
rep("""    SupportBand,
    SupportBandReleasedJoints,
}""","""    SupportBand,
    SupportBandReleasedJoints,
    FoundationHalfAutoJoints,
}""")
rep("    pub const ALL: [Self; 7] = [","    pub const ALL: [Self; 8] = [")
rep("""        Self::SupportBand,
        Self::SupportBandReleasedJoints,
    ];""","""        Self::SupportBand,
        Self::SupportBandReleasedJoints,
        Self::FoundationHalfAutoJoints,
    ];""")
rep("""            Self::SupportBand => "support_band",
            Self::SupportBandReleasedJoints => "support_band_released_joints",
""","""            Self::SupportBand => "support_band",
            Self::SupportBandReleasedJoints => "support_band_released_joints",
            Self::FoundationHalfAutoJoints => "foundation_half_auto_joints",
""")
rep("""        HouseScenario::SupportBand | HouseScenario::SupportBandReleasedJoints => None,
""","""        HouseScenario::SupportBand
        | HouseScenario::SupportBandReleasedJoints
        | HouseScenario::FoundationHalfAutoJoints => None,
""")
p.write_text(s,encoding="utf-8",newline="\n")
print("scenario enum patched")
