//! Greedy surface meshing.
//!
//! For each of the six face directions the chunk is swept as a stack of slices.
//! Each slice becomes a 2D mask of exposed faces, and the mask is consumed into
//! maximal same-material rectangles: grow along `u` first, then grow that strip
//! along `v` while every cell of the strip matches.
//!
//! A solid 16^3 chunk standing alone collapses from 1536 unit faces to 6 quads.
//! Two such chunks side by side drop the 256 faces at their shared seam
//! entirely, because the mask is built from a global-space [`CellSource`] rather
//! than from one volume in isolation.
//!
//! Merging never crosses a material boundary, and the sweep order is fixed, so
//! output is deterministic.
//!
//! [`CellSource`]: engine_core::CellSource

use crate::{CompileStats, Quad, QuadSet, SurfaceCompiler};
use engine_core::{CHUNK_EDGE, CellPos, CellSource, ChunkPos, FaceDir, LocalPos, MaterialId};

const EDGE: usize = CHUNK_EDGE as usize;

/// Merges coplanar, same-material faces into maximal rectangles.
#[derive(Clone, Copy, Default, Debug)]
pub struct GreedyCompiler;

impl SurfaceCompiler for GreedyCompiler {
    fn name(&self) -> &'static str {
        "greedy"
    }

    fn compile(&self, source: &dyn CellSource, chunk: ChunkPos) -> QuadSet {
        let mut quads = Vec::new();
        let mut exposed_faces = 0u32;

        let occupied_cells = LocalPos::iter_all()
            .filter(|local| source.is_occupied(CellPos::from_parts(chunk, *local)))
            .count() as u32;

        // Nothing solid means nothing to sweep.
        if occupied_cells > 0 {
            for dir in FaceDir::ALL {
                let (normal_axis, u_axis, v_axis) = dir.axes();
                let mut mask = [[None::<MaterialId>; EDGE]; EDGE];

                for slice in 0..EDGE {
                    let mut faces_in_slice = 0u32;

                    for (v, row) in mask.iter_mut().enumerate() {
                        for (u, entry) in row.iter_mut().enumerate() {
                            *entry = None;
                            let local = LocalPos::from_axes(
                                (normal_axis, slice as u8),
                                (u_axis, u as u8),
                                (v_axis, v as u8),
                            )
                            .expect("slice indices are within a chunk");
                            let cell = CellPos::from_parts(chunk, local);
                            if let Some(material) = source.material_at(cell)
                                && source.material_at(cell.step(dir)).is_none()
                            {
                                *entry = Some(material);
                                faces_in_slice += 1;
                            }
                        }
                    }

                    exposed_faces += faces_in_slice;
                    if faces_in_slice == 0 {
                        continue;
                    }

                    for v in 0..EDGE {
                        let mut u = 0;
                        while u < EDGE {
                            let Some(material) = mask[v][u] else {
                                u += 1;
                                continue;
                            };

                            // Grow along u.
                            let mut width = 1;
                            while u + width < EDGE && mask[v][u + width] == Some(material) {
                                width += 1;
                            }

                            // Grow the whole strip along v.
                            let mut height = 1;
                            'grow: while v + height < EDGE {
                                for k in 0..width {
                                    if mask[v + height][u + k] != Some(material) {
                                        break 'grow;
                                    }
                                }
                                height += 1;
                            }

                            for row in mask.iter_mut().skip(v).take(height) {
                                for entry in row.iter_mut().skip(u).take(width) {
                                    *entry = None;
                                }
                            }

                            let local = LocalPos::from_axes(
                                (normal_axis, slice as u8),
                                (u_axis, u as u8),
                                (v_axis, v as u8),
                            )
                            .expect("slice indices are within a chunk");
                            quads.push(Quad {
                                dir,
                                origin: CellPos::from_parts(chunk, local),
                                du: width as u32,
                                dv: height as u32,
                                material,
                            });

                            u += width;
                        }
                    }
                }
            }
        }

        QuadSet {
            stats: CompileStats {
                occupied_cells,
                exposed_faces,
                quads: quads.len() as u32,
            },
            quads,
        }
    }
}
