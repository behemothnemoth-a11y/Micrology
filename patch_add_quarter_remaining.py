from pathlib import Path
p=Path(r"C:\Users\behem\.chatgpt-worktrees\Micrology-house-joint-load")/"crates/engine_stress/src/house_scenarios.rs"
s=p.read_text(encoding="utf-8")
def rep(old,new,count=1):
    global s
    assert s.count(old)==count,(old,s.count(old))
    s=s.replace(old,new,count)
rep("""    SupportBandReleasedJoints,
    FoundationHalfAutoJoints,
}""","""    SupportBandReleasedJoints,
    FoundationHalfAutoJoints,
    FoundationQuarterRemainingAutoJoints,
}""")
rep("    pub const ALL: [Self; 8] = [","    pub const ALL: [Self; 9] = [")
rep("""        Self::FoundationHalfAutoJoints,
    ];""","""        Self::FoundationHalfAutoJoints,
        Self::FoundationQuarterRemainingAutoJoints,
    ];""")
rep("""            Self::FoundationHalfAutoJoints => "foundation_half_auto_joints",
""","""            Self::FoundationHalfAutoJoints => "foundation_half_auto_joints",
            Self::FoundationQuarterRemainingAutoJoints => {
                "foundation_quarter_remaining_auto_joints"
            }
""")
rep("""        | HouseScenario::FoundationHalfAutoJoints => None,
""","""        | HouseScenario::FoundationHalfAutoJoints
        | HouseScenario::FoundationQuarterRemainingAutoJoints => None,
""")
rep("""    if scenario == HouseScenario::FoundationHalfAutoJoints {
        // Half the 6.4 x 5.4 m slab is a deliberately large authored edit.
""","""    if matches!(
        scenario,
        HouseScenario::FoundationHalfAutoJoints
            | HouseScenario::FoundationQuarterRemainingAutoJoints
    ) {
        // Foundation-loss diagnostics are deliberately large authored edits.
""")
rep("""    if scenario == HouseScenario::FoundationHalfAutoJoints {
        let targets = house_impact::world_cells(world)
            .into_iter()
            .filter(|(p, material)| {
                *material == reference_house::CONCRETE
                    && p.x >= 64
""","""    if matches!(
        scenario,
        HouseScenario::FoundationHalfAutoJoints
            | HouseScenario::FoundationQuarterRemainingAutoJoints
    ) {
        let min_x = match scenario {
            HouseScenario::FoundationHalfAutoJoints => 64,
            HouseScenario::FoundationQuarterRemainingAutoJoints => 32,
            _ => unreachable!(),
        };
        let targets = house_impact::world_cells(world)
            .into_iter()
            .filter(|(p, material)| {
                *material == reference_house::CONCRETE
                    && p.x >= min_x
""")
rep("""    if scenario != HouseScenario::FoundationHalfAutoJoints {
        return Ok(Vec::new());
    }
""","""    if !matches!(
        scenario,
        HouseScenario::FoundationHalfAutoJoints
            | HouseScenario::FoundationQuarterRemainingAutoJoints
    ) {
        return Ok(Vec::new());
    }
""")
p.write_text(s,encoding="utf-8",newline="\n")
print("quarter-remaining scenario added")
