//! Complex collision
//!
//! Based on UE5.1 source code for collision cooking:
//!   - FChaosDerivedDataCooker
//!   - Chaos::FCookHelper
//!
//! Notes:
//!   - The BVH construction logic here does not match UE5.1 exactly, but should be valid by its
//!     standards and identical for small meshes.

use crate::core::Writer;

pub type Vec3 = [f64; 3];
type Aabb = (Vec3, Vec3);

const OBJECT_TYPE_TRIANGLE_MESH: u8 = 11; // Chaos::ImplicitObjectType::TriangleMesh
const UE_SMALL_NUMBER: f64 = 1.0e-8; // Core/Public/Math/UnrealMathUtility.h:130
const MAX_CHILDREN_IN_LEAF: usize = 22; // TriangleMeshImplicitObject.cpp:1639

// Default (never-explicitly-set) FAABBVectorized() sentinel for an unused BVH child slot.
// AABBVectorized.h:19-23 uses GlobalVectorConstants:: BigNumber = UE_BIG_NUMBER
// (UnrealMathUtility.h:132) = literal 3.4e+38f, NOT FLT_MAX.
const UE_BIG_NUMBER: f32 = 3.4e+38_f32;

fn empty_aabb() -> Aabb {
    (
        [UE_BIG_NUMBER as f64, UE_BIG_NUMBER as f64, UE_BIG_NUMBER as f64],
        [-UE_BIG_NUMBER as f64, -UE_BIG_NUMBER as f64, -UE_BIG_NUMBER as f64],
    )
}

fn w_vec3(w: &mut Writer, v: Vec3) {
    w.f32(v[0] as f32);
    w.f32(v[1] as f32);
    w.f32(v[2] as f32);
}

fn w_aabb(w: &mut Writer, (lo, hi): Aabb) {
    w_vec3(w, lo);
    w_vec3(w, hi);
}

/// Vertex-position hash key that mirrors Python dict-key float equality,
/// where `-0.0 == 0.0` (and therefore hashes identically) -- without this
/// normalization a vertex differing from another only in the sign of a
/// zero component would fail to weld in Rust where it would have welded in
/// the Python original (HashMap keys use bit-for-bit Hash/Eq, not IEEE
/// float equality).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct VKey(u64, u64, u64);

fn vkey(v: Vec3) -> VKey {
    let norm = |x: f64| if x == 0.0 { 0.0_f64 } else { x };
    VKey(norm(v[0]).to_bits(), norm(v[1]).to_bits(), norm(v[2]).to_bits())
}

/// Exact port of `Chaos::CleanTrimesh` (ChaosDerivedDataUtil.cpp:24-195):
/// exact-position vertex welding (WeldThresholdSq=0.0, i.e. only
/// truly-identical positions merge) + degenerate/zero-area triangle removal.
///
/// `indices`: flat list, len == 3*num_triangles (already winding-flipped).
///
/// Returns (unique_vertices, unique_indices, face_remap, vertex_remap):
///   face_remap[i]   = index into the ORIGINAL `indices` triangle list that
///                     survived as the i-th triangle of unique_indices
///   vertex_remap[i] = unique-vertex index that ORIGINAL vertex i merged into
pub fn clean_trimesh(
    vertices: &[Vec3],
    indices: &[u32],
) -> (Vec<Vec3>, Vec<u32>, Vec<usize>, Vec<u32>) {
    let num_source_verts = vertices.len();
    let num_source_tris = indices.len() / 3;
    if num_source_verts == 0 || indices.len() % 3 != 0 {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    let mut vertex_remap = vec![0u32; num_source_verts];
    let mut unique_vertices: Vec<Vec3> = Vec::new();
    let mut first_seen: std::collections::HashMap<VKey, u32> = std::collections::HashMap::new();
    for i in 0..num_source_verts {
        let key = vkey(vertices[i]);
        let existing = match first_seen.get(&key) {
            Some(&idx) => idx,
            None => {
                let idx = unique_vertices.len() as u32;
                unique_vertices.push(vertices[i]);
                first_seen.insert(key, idx);
                idx
            }
        };
        vertex_remap[i] = existing;
    }

    let mut unique_indices: Vec<u32> = Vec::new();
    let mut face_remap: Vec<usize> = Vec::new();
    for orig_tri in 0..num_source_tris {
        let a = indices[orig_tri * 3] as usize;
        let b = indices[orig_tri * 3 + 1] as usize;
        let c = indices[orig_tri * 3 + 2] as usize;
        let ra = vertex_remap[a];
        let rb = vertex_remap[b];
        let rc = vertex_remap[c];
        if ra == rb || ra == rc || rb == rc {
            continue;
        }
        let av = unique_vertices[ra as usize];
        let bv = unique_vertices[rb as usize];
        let cv = unique_vertices[rc as usize];
        let ab = [av[0] - bv[0], av[1] - bv[1], av[2] - bv[2]];
        let ac = [av[0] - cv[0], av[1] - cv[1], av[2] - cv[2]];
        let cross = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let mag_sq = cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2];
        if mag_sq <= UE_SMALL_NUMBER {
            continue;
        }
        unique_indices.extend_from_slice(&[ra, rb, rc]);
        face_remap.push(orig_tri);
    }

    (unique_vertices, unique_indices, face_remap, vertex_remap)
}

/// Applies the bFlipNormals=True winding swap that
/// `UStaticMesh::GetPhysicsTriMeshDataCheckComplex` always sets
/// (StaticMesh.cpp:6999) and `Chaos::Cooking::BuildTriangleMeshes` always
/// honors (ChaosCooking.cpp:227-229): (v0,v1,v2) -> (v1,v0,v2).
pub fn build_trimesh_desc(
    positions: &[Vec3],
    triangles: &[(u32, u32, u32)],
    material_per_triangle: Option<&[u16]>,
) -> (Vec<Vec3>, Vec<u32>, Vec<u16>) {
    let vertices = positions.to_vec();
    let mut flat_indices = Vec::with_capacity(triangles.len() * 3);
    for &(v0, v1, v2) in triangles {
        flat_indices.extend_from_slice(&[v1, v0, v2]);
    }
    let materials = match material_per_triangle {
        Some(m) => m.to_vec(),
        None => vec![0u16; triangles.len()],
    };
    (vertices, flat_indices, materials)
}

fn aabb_of_tri(vertices: &[Vec3], tri_indices: &[u32], tri_id: usize) -> Aabb {
    let a = vertices[tri_indices[tri_id * 3] as usize];
    let b = vertices[tri_indices[tri_id * 3 + 1] as usize];
    let c = vertices[tri_indices[tri_id * 3 + 2] as usize];
    let lo = [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]), a[2].min(b[2]).min(c[2])];
    let hi = [a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1]), a[2].max(b[2]).max(c[2])];
    (lo, hi)
}

fn union_aabb(bounds_list: &[Aabb]) -> Aabb {
    let mut mn = bounds_list[0].0;
    let mut mx = bounds_list[0].1;
    for &(lo, hi) in &bounds_list[1..] {
        for k in 0..3 {
            mn[k] = mn[k].min(lo[k]);
            mx[k] = mx[k].max(hi[k]);
        }
    }
    (mn, mx)
}

pub struct BvhChild {
    pub bounds: Aabb,
    pub child_or_face_index: i32,
    pub face_count: i32,
}

pub type BvhNode = [BvhChild; 2];

/// Builds a valid binary FTrimeshBVH over faces [0, num_triangles).
///
/// Returns (nodes, face_bounds, face_order):
///   face_order: permutation grouping faces contiguously by leaf --
///   callers must reorder MElements/MaterialIndices/ExternalFaceIndexMap
///   to match (UE does the same reordering for cache locality).
pub fn build_bvh(
    vertices: &[Vec3],
    tri_indices: &[u32],
    num_triangles: usize,
) -> (Vec<BvhNode>, Vec<Aabb>, Vec<usize>) {
    let max_children_in_leaf = MAX_CHILDREN_IN_LEAF;
    let face_bounds_by_orig: Vec<Aabb> =
        (0..num_triangles).map(|i| aabb_of_tri(vertices, tri_indices, i)).collect();
    if num_triangles == 0 {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    if num_triangles <= max_children_in_leaf {
        // Single-leaf special case (RebuildFastBVHFromTree's "Nodes.Num()==1"
        // path) -- no split ambiguity, so this is byte-identical to UE's own
        // cook for any mesh this small.
        let root_bounds = union_aabb(&face_bounds_by_orig);
        let node: BvhNode = [
            BvhChild { bounds: root_bounds, child_or_face_index: 0, face_count: num_triangles as i32 },
            BvhChild { bounds: empty_aabb(), child_or_face_index: -1, face_count: 0 },
        ];
        let face_order: Vec<usize> = (0..num_triangles).collect();
        return (vec![node], face_bounds_by_orig, face_order);
    }

    let centroids: Vec<Vec3> = face_bounds_by_orig
        .iter()
        .map(|&(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0])
        .collect();

    let mut nodes: Vec<Option<BvhNode>> = Vec::new();
    let mut face_order: Vec<usize> = Vec::new();

    fn build(
        face_ids: Vec<usize>,
        face_bounds_by_orig: &[Aabb],
        centroids: &[Vec3],
        max_children_in_leaf: usize,
        nodes: &mut Vec<Option<BvhNode>>,
        face_order: &mut Vec<usize>,
    ) -> usize {
        let bounds = union_aabb(&face_ids.iter().map(|&f| face_bounds_by_orig[f]).collect::<Vec<_>>());
        if face_ids.len() <= max_children_in_leaf {
            let start = face_order.len();
            face_order.extend_from_slice(&face_ids);
            let idx = nodes.len();
            nodes.push(Some([
                BvhChild { bounds, child_or_face_index: start as i32, face_count: face_ids.len() as i32 },
                BvhChild { bounds: empty_aabb(), child_or_face_index: -1, face_count: 0 },
            ]));
            return idx;
        }

        let (lo, hi) = bounds;
        let extents = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        let axis = if extents[0] >= extents[1] && extents[0] >= extents[2] {
            0
        } else if extents[1] >= extents[2] {
            1
        } else {
            2
        };
        let mut sorted_ids = face_ids;
        sorted_ids.sort_by(|&a, &b| centroids[a][axis].partial_cmp(&centroids[b][axis]).unwrap());
        let mid = sorted_ids.len() / 2;
        let right_ids = sorted_ids.split_off(mid);
        let left_ids = sorted_ids;

        let idx = nodes.len();
        nodes.push(None);
        let left_bounds = union_aabb(&left_ids.iter().map(|&f| face_bounds_by_orig[f]).collect::<Vec<_>>());
        let right_bounds = union_aabb(&right_ids.iter().map(|&f| face_bounds_by_orig[f]).collect::<Vec<_>>());
        let left_idx = build(left_ids, face_bounds_by_orig, centroids, max_children_in_leaf, nodes, face_order);
        let right_idx = build(right_ids, face_bounds_by_orig, centroids, max_children_in_leaf, nodes, face_order);
        nodes[idx] = Some([
            BvhChild { bounds: left_bounds, child_or_face_index: left_idx as i32, face_count: 0 },
            BvhChild { bounds: right_bounds, child_or_face_index: right_idx as i32, face_count: 0 },
        ]);
        idx
    }

    build(
        (0..num_triangles).collect(),
        &face_bounds_by_orig,
        &centroids,
        max_children_in_leaf,
        &mut nodes,
        &mut face_order,
    );

    let face_bounds: Vec<Aabb> = face_order.iter().map(|&i| face_bounds_by_orig[i]).collect();
    let nodes: Vec<BvhNode> = nodes.into_iter().map(|n| n.unwrap()).collect();
    (nodes, face_bounds, face_order)
}

/// `positions`: render-mesh PositionVertexBuffer.
/// `triangles`: UE winding as authored.
/// `material_per_triangle`: optional, len == triangles.len().
///
/// Returns the raw bytes of a `CookedFormatData["PhysXPC"]` payload.
pub fn encode(positions: &[Vec3], triangles: &[(u32, u32, u32)], material_per_triangle: Option<&[u16]>) -> Vec<u8> {
    let (vertices, flat_indices, tri_materials) = build_trimesh_desc(positions, triangles, material_per_triangle);
    let (unique_vertices, unique_indices, face_remap, vertex_remap) = clean_trimesh(&vertices, &flat_indices);
    let num_triangles = unique_indices.len() / 3;

    let (nodes, face_bounds, face_order) = build_bvh(&unique_vertices, &unique_indices, num_triangles);

    let mut final_tri_indices: Vec<u32> = Vec::with_capacity(num_triangles * 3);
    let mut final_materials: Vec<u16> = Vec::with_capacity(num_triangles);
    let mut final_face_remap: Vec<i32> = Vec::with_capacity(num_triangles);
    for &orig_i in &face_order {
        final_tri_indices.extend_from_slice(&unique_indices[orig_i * 3..orig_i * 3 + 3]);
        final_materials.push(tri_materials[face_remap[orig_i]]);
        final_face_remap.push(face_remap[orig_i] as i32);
    }

    let mut w = Writer::new();
    w.i32(4); // PrecisionSize = sizeof(float) (BuildPrecision is float)
    w.i32(0); // SimpleImplicits.Num -- no AggGeom-derived simple shapes
    w.i32(1); // ComplexImplicits.Num

    w.ubool(true); // TUniquePtr bExists
    w.i32(0); // Tag (first serialized object)
    w.u8(OBJECT_TYPE_TRIANGLE_MESH); // ObjectType

    w.ubool(false); // bIsConvex
    w.ubool(false); // bDoCollide (ctor passes DisableCollisions)
    w.u8(OBJECT_TYPE_TRIANGLE_MESH); // CollisionType

    w.ubool(true); // MParticles.bSerialize
    w.i32(unique_vertices.len() as i32);
    for &v in &unique_vertices {
        w_vec3(&mut w, v);
    }

    let requires_large = unique_vertices.len() >= 0x10000;
    w.ubool(requires_large);
    w.i32(num_triangles as i32);
    for i in 0..num_triangles {
        let a = final_tri_indices[i * 3];
        let b = final_tri_indices[i * 3 + 1];
        let c = final_tri_indices[i * 3 + 2];
        if requires_large {
            w.i32(a as i32);
            w.i32(b as i32);
            w.i32(c as i32);
        } else {
            w.u16(a as u16);
            w.u16(b as u16);
            w.u16(c as u16);
        }
    }

    let (lo, hi) = if num_triangles > 0 {
        union_aabb(&(0..num_triangles).map(|i| aabb_of_tri(&unique_vertices, &final_tri_indices, i)).collect::<Vec<_>>())
    } else {
        ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0])
    };
    w_aabb(&mut w, (lo, hi));

    w.i32(nodes.len() as i32);
    for node in &nodes {
        for child in node {
            w_aabb(&mut w, child.bounds);
            w.i32(child.child_or_face_index);
            w.i32(child.face_count);
        }
    }
    w.i32(face_bounds.len() as i32);
    for &fb in &face_bounds {
        w_aabb(&mut w, fb);
    }

    w.i32(final_materials.len() as i32);
    for &m in &final_materials {
        w.u16(m);
    }
    w.i32(final_face_remap.len() as i32);
    for &f in &final_face_remap {
        w.i32(f);
    }
    w.i32(vertex_remap.len() as i32);
    for &v in &vertex_remap {
        w.i32(v as i32);
    }

    // UVInfo (FBodySetupUVInfo) -- empty (bSupportUVFromHitResults defaults false)
    w.i32(0);
    w.i32(0);
    w.i32(0);
    // Outer Cooker.FaceRemap -- emptied unless bSupportUVsAndFaceRemap is set
    w.i32(0);

    w.into_bytes()
}
