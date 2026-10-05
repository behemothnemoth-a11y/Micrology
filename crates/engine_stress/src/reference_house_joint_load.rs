//! Bounded exact capacity over the reference house's authored construction parts.
//! Diagnostic prototype only: no save schema or global joint law.
use crate::{reference_house, reference_house_joints};
use engine_core::{CellPos, CellSource, FaceDir, MaterialId};
use engine_destruction::{BondKey, BondSite, DamageSpace, FractureState};
use engine_mechanics::{MechanicalRegistry, ReferenceMaterial, load_mode, reference_registry};
use engine_world::World;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub type PartKey = u64;
pub type PartPair = (PartKey, PartKey);

#[derive(Clone, Copy, Debug)]
pub struct AssemblyLimits {
    pub max_parts: usize,
    pub max_interfaces: usize,
    pub max_augmentations: u64,
}
impl Default for AssemblyLimits {
    fn default() -> Self {
        Self {
            max_parts: 512,
            max_interfaces: 4096,
            max_augmentations: 16_384,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PartRecord {
    pub key: PartKey,
    pub material: u32,
    pub label: u32,
    pub cells: u64,
    pub demand_raw: i64,
    pub anchored: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InterfaceRecord {
    pub a: PartKey,
    pub b: PartKey,
    pub faces: u64,
    pub authored_joint: bool,
    pub capacity_a_to_b_raw: i64,
    pub capacity_b_to_a_raw: i64,
    pub authored_bonds: usize,
}
#[derive(Clone, Debug)]
pub struct AssemblyGraph {
    pub parts: BTreeMap<PartKey, PartRecord>,
    pub interfaces: BTreeMap<PartPair, InterfaceRecord>,
    pub authored_bonds: BTreeMap<PartPair, BTreeSet<BondKey>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AssemblyMeasurement {
    pub parts: usize,
    pub interfaces: usize,
    pub augmentations: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AssemblyOutcome {
    Satisfied {
        demand_raw: i64,
        carried_raw: i64,
        measurement: AssemblyMeasurement,
    },
    Overloaded {
        demand_raw: i64,
        carried_raw: i64,
        breakable_pairs: Vec<PartPair>,
        rigid_cut_interfaces: usize,
        measurement: AssemblyMeasurement,
    },
    Deferred {
        reason: String,
        measurement: AssemblyMeasurement,
    },
}

fn key(material: MaterialId, label: u32) -> PartKey {
    (u64::from(material.0) << 32) | u64::from(label)
}
fn structural_material(material: MaterialId) -> bool {
    matches!(
        material,
        reference_house::WOOD
            | reference_house::MASONRY
            | reference_house::CONCRETE
            | reference_house::GLASS
    )
}
pub fn registry() -> MechanicalRegistry {
    reference_registry([
        (ReferenceMaterial::Wood, reference_house::WOOD),
        (ReferenceMaterial::Masonry, reference_house::MASONRY),
        (ReferenceMaterial::Concrete, reference_house::CONCRETE),
        (ReferenceMaterial::Glass, reference_house::GLASS),
    ])
}
fn scaled(raw: i64, milli: u32) -> i64 {
    ((i128::from(raw.max(0)) * i128::from(milli.min(1000))) / 1000).clamp(0, i128::from(i64::MAX))
        as i64
}

/// Build the bounded part graph for `world` as currently damaged.
///
/// `fracture` matters: an authored face whose bond is already broken carries no
/// load and is not a candidate for failing again. Reading the live world while
/// ignoring its damage would credit dead joints with full capacity, so a house
/// that has already lost joints would be reported sounder than it is.
pub fn build(
    world: &World,
    fracture: &FractureState,
    joint_strength_milli: u32,
    cell_length_milli: u32,
    limits: AssemblyLimits,
) -> Result<AssemblyGraph, String> {
    let registry = registry();
    let mut parts = BTreeMap::<PartKey, PartRecord>::new();
    for (vp, volume) in world.volumes_sorted() {
        for (local, material) in volume.iter_occupied() {
            if !structural_material(material) {
                continue;
            }
            let cell = CellPos::from_parts(vp, local);
            let label =
                reference_house_joints::part_label(cell, material).ok_or("missing part label")?;
            let k = key(material, label);
            let profile = registry.get(material).ok_or("missing mechanical profile")?;
            let entry = parts.entry(k).or_insert(PartRecord {
                key: k,
                material: material.0,
                label,
                cells: 0,
                demand_raw: 0,
                anchored: false,
            });
            entry.cells = entry.cells.saturating_add(1);
            // The cell solver's demand/capacity ratio is its one-unit-cell
            // reference. Under geometric similarity, weight scales with L^3
            // and face capacity with L^2, so the ratio scales with cell edge L.
            // The 50 mm house therefore contributes 50/1000 of that reference
            // demand. This is dimensional scaling, not SI calibration.
            let demand = if material == reference_house::GLASS {
                0
            } else {
                scaled(profile.density.raw(), cell_length_milli)
            };
            entry.demand_raw = entry.demand_raw.saturating_add(demand);
            entry.anchored |= world.is_anchor(cell);
            if parts.len() > limits.max_parts {
                return Err("part budget".into());
            }
        }
    }

    let authored = reference_house_joints::authored_joint_bonds(world);
    let mut interfaces = BTreeMap::<PartPair, InterfaceRecord>::new();
    let mut authored_bonds = BTreeMap::<PartPair, BTreeSet<BondKey>>::new();
    for (vp, volume) in world.volumes_sorted() {
        for (local, material) in volume.iter_occupied() {
            if !structural_material(material) {
                continue;
            }
            let cell = CellPos::from_parts(vp, local);
            let label =
                reference_house_joints::part_label(cell, material).ok_or("missing label")?;
            let ka = key(material, label);
            let pa = registry.get(material).ok_or("profile a")?;
            for dir in [FaceDir::PosX, FaceDir::PosY, FaceDir::PosZ] {
                let Some(other) = cell.checked_step(dir) else {
                    continue;
                };
                let Some(other_material) = world.material_at(other) else {
                    continue;
                };
                if !structural_material(other_material) {
                    continue;
                }
                let other_label = reference_house_joints::part_label(other, other_material)
                    .ok_or("missing other label")?;
                let kb = key(other_material, other_label);
                if ka == kb {
                    continue;
                }
                let bond = BondKey::new(cell, dir).ok_or("bond")?;
                let pair = if ka < kb { (ka, kb) } else { (kb, ka) };
                let pb = registry.get(other_material).ok_or("profile b")?;
                let is_authored = authored.contains(&bond);
                let broken = fracture
                    .integrity(BondSite {
                        space: DamageSpace::StaticWorld,
                        bond,
                    })
                    .0
                    == 0;
                let factor = if is_authored {
                    joint_strength_milli
                } else {
                    1000
                };
                // Glazing is represented in the graph only so occupancy cannot
                // become a hidden structural bridge. It contributes no house load
                // and carries no construction load in this diagnostic.
                let (ab, ba) = if material == reference_house::GLASS
                    || other_material == reference_house::GLASS
                    || broken
                {
                    (0, 0)
                } else {
                    (
                        scaled(
                            pa.capacity(load_mode(cell, other))
                                .min(pb.capacity(load_mode(cell, other)))
                                .raw(),
                            factor,
                        ),
                        scaled(
                            pb.capacity(load_mode(other, cell))
                                .min(pa.capacity(load_mode(other, cell)))
                                .raw(),
                            factor,
                        ),
                    )
                };
                let e = interfaces.entry(pair).or_insert(InterfaceRecord {
                    a: pair.0,
                    b: pair.1,
                    faces: 0,
                    authored_joint: false,
                    capacity_a_to_b_raw: 0,
                    capacity_b_to_a_raw: 0,
                    authored_bonds: 0,
                });
                e.faces = e.faces.saturating_add(1);
                e.authored_joint |= is_authored;
                if ka == pair.0 {
                    e.capacity_a_to_b_raw = e.capacity_a_to_b_raw.saturating_add(ab);
                    e.capacity_b_to_a_raw = e.capacity_b_to_a_raw.saturating_add(ba);
                } else {
                    e.capacity_b_to_a_raw = e.capacity_b_to_a_raw.saturating_add(ab);
                    e.capacity_a_to_b_raw = e.capacity_a_to_b_raw.saturating_add(ba);
                }
                // Only a live authored bond can still be failed by a later
                // generation; a dead one is already spent.
                if is_authored && !broken {
                    authored_bonds.entry(pair).or_default().insert(bond);
                    e.authored_bonds += 1;
                }
                if interfaces.len() > limits.max_interfaces {
                    return Err("interface budget".into());
                }
            }
        }
    }
    Ok(AssemblyGraph {
        parts,
        interfaces,
        authored_bonds,
    })
}

#[derive(Clone, Copy)]
struct Edge {
    to: usize,
    cap: i64,
    flow: i64,
}
struct Network {
    edges: Vec<Edge>,
    adj: Vec<Vec<usize>>,
}
impl Network {
    fn new(n: usize) -> Self {
        Self {
            edges: Vec::new(),
            adj: vec![Vec::new(); n],
        }
    }
    fn add(&mut self, from: usize, to: usize, cap: i64) {
        let i = self.edges.len();
        self.edges.push(Edge {
            to,
            cap: cap.max(0),
            flow: 0,
        });
        self.adj[from].push(i);
        self.edges.push(Edge {
            to: from,
            cap: 0,
            flow: 0,
        });
        self.adj[to].push(i + 1);
    }
    fn residual(&self, i: usize) -> i64 {
        self.edges[i].cap.saturating_sub(self.edges[i].flow)
    }
}

pub fn evaluate(graph: &AssemblyGraph, limits: AssemblyLimits) -> AssemblyOutcome {
    let keys: Vec<_> = graph.parts.keys().copied().collect();
    let n = keys.len();
    let source = n;
    let sink = n + 1;
    let measurement0 = AssemblyMeasurement {
        parts: n,
        interfaces: graph.interfaces.len(),
        augmentations: 0,
    };
    if n > limits.max_parts || graph.interfaces.len() > limits.max_interfaces {
        return AssemblyOutcome::Deferred {
            reason: "graph budget".into(),
            measurement: measurement0,
        };
    }
    let idx: BTreeMap<_, _> = keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let mut net = Network::new(n + 2);
    let mut demand = 0i64;
    for k in &keys {
        let p = &graph.parts[k];
        let i = idx[k];
        demand = demand.saturating_add(p.demand_raw.max(0));
        net.add(source, i, p.demand_raw.max(0));
        if p.anchored {
            net.add(i, sink, i64::MAX / 8);
        }
    }
    for (pair, e) in &graph.interfaces {
        let a = idx[&pair.0];
        let b = idx[&pair.1];
        net.add(a, b, e.capacity_a_to_b_raw);
        net.add(b, a, e.capacity_b_to_a_raw);
    }
    let mut carried = 0i64;
    let mut aug = 0u64;
    loop {
        let mut prev = vec![None; n + 2];
        let mut q = VecDeque::from([source]);
        prev[source] = Some(usize::MAX);
        while let Some(v) = q.pop_front() {
            if v == sink {
                break;
            }
            for &ei in &net.adj[v] {
                let to = net.edges[ei].to;
                if prev[to].is_none() && net.residual(ei) > 0 {
                    prev[to] = Some(ei);
                    q.push_back(to);
                }
            }
        }
        if prev[sink].is_none() {
            break;
        }
        if aug >= limits.max_augmentations {
            return AssemblyOutcome::Deferred {
                reason: "augmentation budget".into(),
                measurement: AssemblyMeasurement {
                    parts: n,
                    interfaces: graph.interfaces.len(),
                    augmentations: aug,
                },
            };
        }
        let mut add = i64::MAX;
        let mut v = sink;
        while v != source {
            let ei = prev[v].unwrap();
            add = add.min(net.residual(ei));
            v = net.edges[ei ^ 1].to;
        }
        v = sink;
        while v != source {
            let ei = prev[v].unwrap();
            net.edges[ei].flow = net.edges[ei].flow.saturating_add(add);
            net.edges[ei ^ 1].flow = net.edges[ei ^ 1].flow.saturating_sub(add);
            v = net.edges[ei ^ 1].to;
        }
        carried = carried.saturating_add(add);
        aug += 1;
    }
    let measurement = AssemblyMeasurement {
        parts: n,
        interfaces: graph.interfaces.len(),
        augmentations: aug,
    };
    if carried >= demand {
        return AssemblyOutcome::Satisfied {
            demand_raw: demand,
            carried_raw: carried,
            measurement,
        };
    }

    let mut reach = vec![false; n + 2];
    let mut q = VecDeque::from([source]);
    reach[source] = true;
    while let Some(v) = q.pop_front() {
        for &ei in &net.adj[v] {
            let to = net.edges[ei].to;
            if !reach[to] && net.residual(ei) > 0 {
                reach[to] = true;
                q.push_back(to);
            }
        }
    }
    let mut breakable = BTreeSet::new();
    let mut rigid = 0usize;
    for (pair, e) in &graph.interfaces {
        let a = idx[&pair.0];
        let b = idx[&pair.1];
        let separated = reach[a] != reach[b];
        if e.authored_joint && separated {
            // A zero-capacity decorative interface (notably glass) still has to
            // be cut topologically when the load-bearing parts land on opposite
            // sides of the mechanical cut, or occupancy would weld them back.
            breakable.insert(*pair);
        } else {
            let carries_cut_load = (reach[a] && !reach[b] && e.capacity_a_to_b_raw > 0)
                || (reach[b] && !reach[a] && e.capacity_b_to_a_raw > 0);
            if carries_cut_load {
                rigid += 1;
            }
        }
    }
    AssemblyOutcome::Overloaded {
        demand_raw: demand,
        carried_raw: carried,
        breakable_pairs: breakable.into_iter().collect(),
        rigid_cut_interfaces: rigid,
        measurement,
    }
}

pub fn bonds_for_pairs(graph: &AssemblyGraph, pairs: &[PartPair]) -> BTreeSet<BondKey> {
    pairs
        .iter()
        .flat_map(|p| {
            graph
                .authored_bonds
                .get(p)
                .into_iter()
                .flat_map(|s| s.iter().copied())
        })
        .collect()
}
