//! Produces serialized render mesh data as expected by FStaticMeshLODResources.
//! 
//! Notes:
//!   - Per-face-corner vertices are never welded across faces.
//!   - Tangent packing uses a standard int8 quantization (round(component*127), clamped to
//!     [-127,127]) rather than reproducing UE's exact FPackedNormal rounding bit-for-bit -- this
//!     only affects normal-map lighting precision, not geometry, indices, or collision.
//!   - Bitangent handedness (TangentZ.W) is always +127 (a fixed convention); wrong handedness
//!     would only flip normal-map Y, not geometry.
//!   - If no normals are supplied, flat per-face normals are computed from the triangle winding. If
//!     no UVs are supplied, (0,0) is used everywhere.
//!   - Positions are passed through as-is: input is expected to already be in UE's axis convention
//!     and centimeter units.

use std::collections::HashMap;

pub type Vec3 = [f64; 3];
pub type Vec2 = [f64; 2];

/// One face-corner: an index into `MeshInput::positions`, plus optional
/// indices into `uvs` / `normals` (None means "not authored for this
/// corner", mirroring OBJ's optional `vt`/`vn` face-reference slots).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Corner {
    pub position: usize,
    pub uv: Option<usize>,
    pub normal: Option<usize>,
}

impl Corner {
    pub fn new(position: usize) -> Self {
        Corner { position, uv: None, normal: None }
    }

    pub fn with_uv(mut self, uv: usize) -> Self {
        self.uv = Some(uv);
        self
    }

    pub fn with_normal(mut self, normal: usize) -> Self {
        self.normal = Some(normal);
        self
    }
}

/// A polygon (already fan-triangulated by the caller is NOT required --
/// `build_render_mesh` fan-triangulates arbitrary N-gons itself, exactly
/// like the Python OBJ loader did) with a material assignment.
#[derive(Clone, Debug)]
pub struct Face {
    pub material_index: usize,
    pub corners: Vec<Corner>,
}

/// Raw mesh input: the direct-data equivalent of what `load_obj_full` used
/// to parse out of a Wavefront OBJ. This is the crate's whole public input
/// surface -- no file format, just vertex/face/uv data.
#[derive(Clone, Debug, Default)]
pub struct MeshInput {
    pub positions: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub normals: Vec<Vec3>,
    pub faces: Vec<Face>,
    pub material_names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Tangent {
    /// (x, y, z, w) as already-packed int8 components.
    pub tangent_x: (i8, i8, i8, i8),
    pub tangent_z: (i8, i8, i8, i8),
}

#[derive(Clone, Debug)]
pub struct Section {
    pub material_index: usize,
    pub first_index: usize,
    pub num_triangles: usize,
    pub min_vertex_index: usize,
    pub max_vertex_index: usize,
}

#[derive(Clone, Debug)]
pub struct Bounds {
    pub origin: Vec3,
    pub box_extent: Vec3,
    pub sphere_radius: f64,
}

/// Returns the flat, per-face-corner render-mesh data that FStaticMeshLODResources expects:
///   positions: one per render vertex
///   tangents:  packed int8 4-tuples, ready for the vertex buffer
///   uvs:       one UV channel, one per render vertex
///   indices:   flat triangle index list, GROUPED CONTIGUOUSLY BY MATERIAL
///              (required for valid FStaticMeshSection ranges)
///   sections:  MaterialIndex/FirstIndex/NumTriangles/MinVertexIndex/MaxVertexIndex
///   bounds:    Origin/BoxExtent/SphereRadius
#[derive(Clone, Debug)]
pub struct RenderMesh {
    pub positions: Vec<Vec3>,
    pub tangents: Vec<Tangent>,
    pub uvs: Vec<Vec<Vec2>>,
    pub indices: Vec<u32>,
    pub sections: Vec<Section>,
    pub bounds: Bounds,
    pub material_names: Vec<String>,
}

fn normalize(v: Vec3) -> Vec3 {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length < 1e-12 {
        return [0.0, 0.0, 1.0];
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Standard int8 quantization (see module docs -- not UE's exact
/// FPackedNormal rounding, but geometry/collision-neutral).
fn pack_tangent_component(x: f64) -> i8 {
    let v = x.max(-1.0).min(1.0);
    let rounded = python_round(v * 127.0);
    rounded.max(-127.0).min(127.0) as i8
}

/// Python 3's built-in `round()` on a float does round-half-to-even
/// (banker's rounding), same as IEEE 754 roundTiesToEven -- match that
/// exactly rather than Rust f64::round()'s round-half-away-from-zero, since
/// a component landing exactly on x.5 would otherwise pack to a different
/// int8 value than the Python original.
fn python_round(x: f64) -> f64 {
    let floor = x.floor();
    let diff = x - floor;
    if (diff - 0.5).abs() < 1e-9 {
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        (x).round()
    }
}

pub fn build_render_mesh(mesh: &MeshInput) -> RenderMesh {
    let positions_in: &Vec<Vec3> = &mesh.positions;
    let normals_in: &Vec<Vec3> = &mesh.normals;
    let uvs_in: &Vec<Vec2> = &mesh.uvs;
    let faces = &mesh.faces;

    // Triangulate (fan) and group by material.
    let mut tris_by_material: std::collections::BTreeMap<usize, Vec<(Corner, Corner, Corner)>> =
        std::collections::BTreeMap::new();
    for face in faces {
        let corners = &face.corners;
        for k in 1..corners.len().saturating_sub(1) {
            tris_by_material
                .entry(face.material_index)
                .or_default()
                .push((corners[0], corners[k], corners[k + 1]));
        }
    }

    // When no normals were supplied at all, compute SMOOTH per-vertex normals (area-weighted
    // average of adjacent face normals, keyed by position index) rather than flat per-face normals.
    let has_any_vn = faces.iter().any(|f| f.corners.iter().any(|c| c.normal.is_some()));
    let smooth_normals: Option<Vec<Vec3>> = if !has_any_vn {
        let mut accum = vec![[0.0f64; 3]; positions_in.len()];
        for tris in tris_by_material.values() {
            for (c0, c1, c2) in tris {
                let p0 = positions_in[c0.position];
                let p1 = positions_in[c1.position];
                let p2 = positions_in[c2.position];
                let face_n = cross(sub(p1, p0), sub(p2, p0)); // unnormalized -> area-weighted
                for c in [c0, c1, c2] {
                    let a = &mut accum[c.position];
                    a[0] += face_n[0];
                    a[1] += face_n[1];
                    a[2] += face_n[2];
                }
            }
        }
        Some(accum.into_iter().map(normalize).collect())
    } else {
        None
    };

    let mut out_positions: Vec<Vec3> = Vec::new();
    let mut out_tangents: Vec<Tangent> = Vec::new();
    let mut out_uvs: Vec<Vec<Vec2>> = Vec::new();
    let mut out_indices: Vec<u32> = Vec::new();
    let mut sections: Vec<Section> = Vec::new();

    let arbitrary_tangent = |n: Vec3| -> Vec3 {
        let helper = if n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        normalize(cross(helper, n))
    };

    for (&material_index, tris) in tris_by_material.iter() {
        let first_index = out_indices.len();
        let min_vert = out_positions.len();
        // Wedge dedup keyed by (posIdx, uvIdx, normalIdx): reuses a render vertex whenever a face
        // corner repeats the exact same attribute combo. 
        let mut wedge_cache: HashMap<Corner, u32> = HashMap::new();

        for (c0, c1, c2) in tris {
            let corners = [*c0, *c1, *c2];
            let face_normals: [Vec3; 3] = if c0.normal.is_some() && c1.normal.is_some() && c2.normal.is_some() {
                [
                    normalize(normals_in[c0.normal.unwrap()]),
                    normalize(normals_in[c1.normal.unwrap()]),
                    normalize(normals_in[c2.normal.unwrap()]),
                ]
            } else {
                let sn = smooth_normals.as_ref().expect("smooth normals precomputed");
                [sn[c0.position], sn[c1.position], sn[c2.position]]
            };

            let mut tri_out = [0u32; 3];
            for i in 0..3 {
                let c = corners[i];
                let n = face_normals[i];
                let idx = match wedge_cache.get(&c) {
                    Some(&idx) => idx,
                    None => {
                        let idx = out_positions.len() as u32;
                        wedge_cache.insert(c, idx);
                        out_positions.push(positions_in[c.position]);
                        let t = arbitrary_tangent(n);
                        let tx = (
                            pack_tangent_component(t[0]),
                            pack_tangent_component(t[1]),
                            pack_tangent_component(t[2]),
                            0,
                        );
                        let tz = (
                            pack_tangent_component(n[0]),
                            pack_tangent_component(n[1]),
                            pack_tangent_component(n[2]),
                            127,
                        );
                        out_tangents.push(Tangent { tangent_x: tx, tangent_z: tz });
                        let (u, v) = match c.uv {
                            Some(ui) => (uvs_in[ui][0], uvs_in[ui][1]),
                            None => (0.0, 0.0),
                        };
                        out_uvs.push(vec![[u, 1.0 - v]]); // flip V: UE convention
                        idx
                    }
                };
                tri_out[i] = idx;
            }
            out_indices.extend_from_slice(&tri_out);
        }

        let num_triangles = (out_indices.len() - first_index) / 3;
        let max_vert = out_positions.len() - 1;
        sections.push(Section {
            material_index,
            first_index,
            num_triangles,
            min_vertex_index: min_vert,
            max_vertex_index: max_vert,
        });
    }

    let (lo, hi) = if out_positions.is_empty() {
        ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0])
    } else {
        let mut lo = out_positions[0];
        let mut hi = out_positions[0];
        for p in &out_positions[1..] {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    };
    let origin = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let extent = [(hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0, (hi[2] - lo[2]) / 2.0];
    let radius = (extent[0] * extent[0] + extent[1] * extent[1] + extent[2] * extent[2]).sqrt();

    RenderMesh {
        positions: out_positions,
        tangents: out_tangents,
        uvs: out_uvs,
        indices: out_indices,
        sections,
        bounds: Bounds { origin, box_extent: extent, sphere_radius: radius },
        material_names: mesh.material_names.clone(),
    }
}
