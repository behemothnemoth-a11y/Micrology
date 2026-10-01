//! Exact surface extraction — the engine's topology oracle.
//!
//! One unit quad per exposed face, no merging, no cleverness. Slow and large by
//! design: its job is to be obviously correct so that the optimised mesher has
//! something trustworthy to be checked against.
//!
//! This implementation is deliberately written independently of
//! [`GreedyCompiler`](crate::GreedyCompiler) rather than sharing its slice
//! machinery — an oracle that shares code with the thing it validates is not an
//! oracle.

use crate::{CompileStats, Quad, QuadSet, SurfaceCompiler};
use engine_core::{CellPos, CellSource, FaceDir, LocalPos, VolumePos};

/// Emits every exposed cell face as its own quad.
#[derive(Clone, Copy, Default, Debug)]
pub struct ExactCompiler;

impl SurfaceCompiler for ExactCompiler {
    fn name(&self) -> &'static str {
        "exact"
    }

    fn compile(&self, source: &dyn CellSource, chunk: VolumePos) -> QuadSet {
        let mut quads = Vec::new();
        let mut occupied_cells = 0;

        for local in LocalPos::iter_all() {
            let cell = CellPos::from_parts(chunk, local);
            let Some(material) = source.material_at(cell) else {
                continue;
            };
            occupied_cells += 1;

            for dir in FaceDir::ALL {
                // A face is exposed when the cell beyond it is empty. Note this
                // samples outside the chunk on boundaries, so a face shared with
                // a solid neighbouring chunk is correctly dropped.
                if source.material_at(cell.step(dir)).is_none() {
                    quads.push(Quad::unit(dir, cell, material));
                }
            }
        }

        let exposed_faces = quads.len() as u32;
        QuadSet {
            quads,
            stats: CompileStats {
                occupied_cells,
                exposed_faces,
                quads: exposed_faces,
            },
        }
    }
}
