from pathlib import Path
root=Path(r"C:\Users\behem\.chatgpt-worktrees\Micrology-house-joint-load")

def patch(rel, reps):
    p=root/rel
    s=p.read_text(encoding="utf-8")
    for old,new,count in reps:
        assert s.count(old)==count,(rel,old[:80],s.count(old))
        s=s.replace(old,new,count)
    p.write_text(s,encoding="utf-8",newline="\n")

patch("apps/sandbox/src/fracture_worker.rs",[
("""    pub static_volume: Option<(engine_core::VolumePos, Option<Revision>)>,
}""","""    pub static_volume: Option<(engine_core::VolumePos, Option<Revision>)>,
    /// Optional one-request override. None uses the host's normal bounded caps.
    pub limits: Option<FractureJobLimits>,
}""",1),
("""            static_volume: None,
        });""","""            static_volume: None,
            limits: None,
        });""",1),
("""    let input = FractureJobInput::capture(
        &world.0,
        fragments.store_ref(),
        &lab.fracture_state,
        destruction.sequence,
        request.space,
        known,
        lab.background.job_limits,
    );""","""    let job_limits = request.limits.unwrap_or(lab.background.job_limits);
    let input = FractureJobInput::capture(
        &world.0,
        fragments.store_ref(),
        &lab.fracture_state,
        destruction.sequence,
        request.space,
        known,
        job_limits,
    );""",1),
("""                static_volume: None
            }));""","""                static_volume: None,
                limits: None
            }));""",1),
("""            static_volume: None
        }));""","""            static_volume: None,
            limits: None
        }));""",1),
])
patch("apps/sandbox/src/contact_fracture.rs",[
("""                static_volume: hit.static_volume,
            });""","""                static_volume: hit.static_volume,
                limits: None,
            });""",1),
])
patch("apps/sandbox/src/interaction.rs",[
("""                static_volume: None,
            });""","""                static_volume: None,
                limits: None,
            });""",1),
])
patch("apps/sandbox/src/sim_lab.rs",[
("""        static_volume: None,
    }) {""","""        static_volume: None,
        limits: None,
    }) {""",1),
("""            static_volume: None,
        });""","""            static_volume: None,
            limits: None,
        });""",1),
("""    if !lab.background.enqueue(crate::fracture_worker::Request {
        work,
        effect,
        space: DamageSpace::StaticWorld,
        geometry: None,
        static_volume: None,
    }) {""","""    let mut request_limits = lab.background.job_limits;
    request_limits.transaction.fracture.max_cells_visited =
        engine_stress::house_scenarios::limits_for(scenario)
            .transaction
            .fracture
            .max_cells_visited;
    if !lab.background.enqueue(crate::fracture_worker::Request {
        work,
        effect,
        space: DamageSpace::StaticWorld,
        geometry: None,
        static_volume: None,
        limits: Some(request_limits),
    }) {""",1),
])
print("request-local fracture limits patched")
