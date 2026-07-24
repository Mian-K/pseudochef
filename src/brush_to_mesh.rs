use glam::{Mat3, Quat, Vec3};
use itertools::Itertools;

fn vec3(p: shalrath::repr::Point) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}
fn calculate_normal(plane: shalrath::repr::TrianglePlane) -> Vec3 {
    let p0 = vec3(plane.v0);
    let p1 = vec3(plane.v1);
    let p2 = vec3(plane.v2);
    let a = p2 - p0;
    let b = p1 - p0;
    a.cross(b)
}
fn flip_y(point: &mut shalrath::repr::Point) {
    point.y = -point.y;
}
fn mirror_xz(brush: &shalrath::repr::Brush) -> shalrath::repr::Brush {
    let mut planes = vec![];
    for plane in &brush.0 {
        let mut mirrored = plane.clone();
        flip_y(&mut mirrored.plane.v0);
        flip_y(&mut mirrored.plane.v1);
        flip_y(&mut mirrored.plane.v2);
        planes.push(mirrored);
    }
    shalrath::repr::Brush::new(planes)
}
pub fn convert_to_mesh(brush: &shalrath::repr::Brush) -> pseudocooker::MeshInput {
    // TrenchBroom uses a right-handed coordinate system with +z pointing up.
    // Unreal uses a left-handed coordinate system with +z pointing up.
    // So, we mirror across the xz-plane when converting.
    let brush = mirror_xz(&brush);

    let mut vertices = vec![vec![]; brush.0.len()];
    let mut normals = vec![];
    for plane in &brush.0 {
        normals.push(calculate_normal(plane.plane));
    }
    let planes_and_normals: Vec<_> = brush
        .0
        .iter()
        .enumerate()
        .map(|(i, plane)| (i, plane.plane, calculate_normal(plane.plane)))
        .collect();
    // Determinant threshold for treating three plane normals as linearly independent
    // (i.e. the planes meet at a unique point rather than being parallel/degenerate).
    const DET_EPSILON: f32 = 1e-6;
    // World-space distance threshold used both to decide whether a candidate point
    // actually lies inside every plane's half-space, and to dedupe vertices that
    // more than 3 planes meet at (e.g. a chamfered corner or a pyramid apex).
    const VERTEX_EPSILON: f32 = 1e-2;
    // `calculate_normal`'s sign convention (outward vs. inward) depends on the
    // winding of each plane's 3 points, which flips uniformly for every face
    // when the brush is mirrored above. Rather than hardcode which convention
    // applies post-mirror, derive which side of each plane is "interior" from
    // a point that's roughly in the middle of the brush.
    let interior_reference: Vec3 = planes_and_normals.iter().map(|(_, p, _)| vec3(p.v0)).sum::<Vec3>()
        / planes_and_normals.len() as f32;
    let interior_signs: Vec<f32> = planes_and_normals
        .iter()
        .map(|(_, p, n)| {
            let x = vec3(p.v0);
            // Choose the sign so that `(point - x).dot(n) * sign` is <= 0 for the
            // interior reference point, i.e. so margin <= 0 means "interior side".
            if (interior_reference - x).dot(*n) >= 0.0 { -1.0 } else { 1.0 }
        })
        .collect();
    for c in planes_and_normals.iter().combinations(3) {
        let (i0, p0, n0) = c[0];
        let (i1, p1, n1) = c[1];
        let (i2, p2, n2) = c[2];
        let x0 = vec3(p0.v0);
        let x1 = vec3(p1.v0);
        let x2 = vec3(p2.v0);
        let u0 = n0.normalize();
        let u1 = n1.normalize();
        let u2 = n2.normalize();
        let det = Mat3::from_cols(u0, u1, u2).determinant();
        if det.abs() < DET_EPSILON {
            // Normals are (near-)parallel: these three planes don't meet at a
            // single point, so there's nothing to intersect.
            continue;
        }
        let c0 = x0.dot(u0) * u1.cross(u2);
        let c1 = x1.dot(u1) * u2.cross(u0);
        let c2 = x2.dot(u2) * u0.cross(u1);
        let intersection = det.powi(-1) * (c0 + c1 + c2);

        // A candidate point is only a real vertex of the (convex) brush if it
        // lies on the interior side of every plane, not just the three we
        // solved with. This is what makes the intersection test work for any
        // convex brush, not just ones where faces meet at right angles.
        let is_vertex = planes_and_normals
            .iter()
            .zip(interior_signs.iter())
            .all(|((_, plane, normal), sign)| {
                let x = vec3(plane.v0);
                let n = normal.normalize();
                (intersection - x).dot(n) * sign <= VERTEX_EPSILON
            });
        if !is_vertex {
            continue;
        }

        for &i in &[*i0, *i1, *i2] {
            let face_vertices = &mut vertices[i];
            let is_duplicate = face_vertices
                .iter()
                .any(|v: &Vec3| v.distance(intersection) < VERTEX_EPSILON);
            if !is_duplicate {
                face_vertices.push(intersection);
            }
        }
    }

    let mut positions = vec![];
    let mut faces = vec![];
    for (normal, face_vertices) in normals.iter().zip(vertices.iter()) {
        let base = positions.len();
        let quat = Quat::from_rotation_arc(normal.clone(), Vec3::Z);
        let mut polar_angles = vec![];
        for (i, vertex) in face_vertices.iter().enumerate() {
            positions.push([vertex.x as f64, vertex.y as f64, vertex.z as f64]);
            let rv = quat * vertex;
            let theta = ordered_float::OrderedFloat(rv.y.atan2(rv.x));
            polar_angles.push((theta, i));
        }
        polar_angles.sort();
        let mut tris = vec![];
        for i in 1..polar_angles.len() - 1 {
            let a = polar_angles[0].1;
            let b = polar_angles[i].1;
            let c = polar_angles[i + 1].1;
            // TODO unhardcode material_index; would need to pass in whole entity, not just brush
            let f = pseudocooker::Face {
                material_index: 0,
                corners: vec![
                    pseudocooker::Corner::new(base + a),
                    pseudocooker::Corner::new(base + b),
                    pseudocooker::Corner::new(base + c),
                ],
            };
            tris.push(f);
        }
        faces.extend(tris);
    }

    pseudocooker::MeshInput {
        positions,
        uvs: Vec::new(),
        normals: Vec::new(),
        faces,
        material_names: vec![],
    }
    // TODO break up each face for more even vertex lighting
}

#[cfg(test)]
mod tests {
    use super::*;
    use shalrath::repr::{Brush, BrushPlane, Point, TrianglePlane};

    fn point(x: f32, y: f32, z: f32) -> Point {
        Point { x, y, z }
    }

    // Builds a plane from 3 points, flipping the winding order if needed so the
    // resulting normal points away from `centroid` (i.e. outward, as brush face
    // normals are expected to be).
    fn plane_outward(v0: Point, v1: Point, v2: Point, centroid: Vec3) -> BrushPlane {
        let normal = calculate_normal(TrianglePlane { v0, v1, v2 });
        let (v0, v1, v2) = if normal.dot(vec3(v0) - centroid) < 0.0 {
            (v0, v2, v1)
        } else {
            (v0, v1, v2)
        };
        BrushPlane {
            plane: TrianglePlane { v0, v1, v2 },
            ..Default::default()
        }
    }

    #[test]
    fn convert_to_mesh_handles_orthogonal_box() {
        // A plain axis-aligned box: every vertex is formed by 3 mutually
        // perpendicular faces, the one case the old angle-based gate handled.
        let centroid = Vec3::new(5.0, 5.0, 5.0);
        let planes = vec![
            plane_outward(point(0.0, 0.0, 0.0), point(0.0, 10.0, 0.0), point(10.0, 0.0, 0.0), centroid),
            plane_outward(point(0.0, 0.0, 10.0), point(10.0, 0.0, 10.0), point(0.0, 10.0, 10.0), centroid),
            plane_outward(point(0.0, 0.0, 0.0), point(10.0, 0.0, 0.0), point(0.0, 0.0, 10.0), centroid),
            plane_outward(point(0.0, 10.0, 0.0), point(0.0, 10.0, 10.0), point(10.0, 10.0, 0.0), centroid),
            plane_outward(point(0.0, 0.0, 0.0), point(0.0, 0.0, 10.0), point(0.0, 10.0, 0.0), centroid),
            plane_outward(point(10.0, 0.0, 0.0), point(10.0, 10.0, 0.0), point(10.0, 0.0, 10.0), centroid),
        ];
        let brush = Brush::new(planes);
        let mesh = convert_to_mesh(&brush);

        // 6 quad faces, fan-triangulated into 2 triangles each.
        assert_eq!(mesh.faces.len(), 12);
        for p in &mesh.positions {
            assert!(p.iter().all(|c| c.is_finite()));
        }
    }

    #[test]
    fn convert_to_mesh_handles_non_orthogonal_wedge() {
        // A triangular prism (right triangle cross-section extruded along z).
        // Its two triangular caps meet the two axis-aligned side faces at right
        // angles, but the slanted hypotenuse face meets everything else at
        // non-right angles, and every prior corner of the prism relies on it.
        // The old angle-based gate could only ever find the 2 vertices where
        // the hypotenuse face isn't involved; it would silently drop the other
        // 4 vertices of the shape.
        let centroid = Vec3::new(10.0 / 3.0, 10.0 / 3.0, 5.0);
        let a = point(0.0, 0.0, 0.0);
        let b = point(10.0, 0.0, 0.0);
        let c = point(0.0, 10.0, 0.0);
        let a2 = point(0.0, 0.0, 10.0);
        let b2 = point(10.0, 0.0, 10.0);
        let c2 = point(0.0, 10.0, 10.0);

        let planes = vec![
            plane_outward(a, b, c, centroid),    // bottom cap
            plane_outward(a2, c2, b2, centroid), // top cap
            plane_outward(a, a2, b2, centroid),  // side face along AB (y = 0)
            plane_outward(a, c, c2, centroid),   // side face along AC (x = 0)
            plane_outward(b, b2, c2, centroid),  // hypotenuse side face along BC
        ];
        let brush = Brush::new(planes);
        let mesh = convert_to_mesh(&brush);

        // 2 triangular caps (1 tri each) + 3 rectangular sides (2 tris each).
        assert_eq!(mesh.faces.len(), 8);
        for p in &mesh.positions {
            assert!(p.iter().all(|c| c.is_finite()));
        }
    }
}
