//! Writes a [`pseudocooker::MeshInput`] out as an OBJ file for visual inspection in e.g. Blender.

use pseudocooker::MeshInput;
use std::io::{self, Write};
use std::path::Path;

pub fn write_obj<W: Write>(mesh: &MeshInput, writer: &mut W) -> io::Result<()> {
    for p in &mesh.positions {
        writeln!(writer, "v {} {} {}", p[0], p[1], p[2])?;
    }
    for uv in &mesh.uvs {
        writeln!(writer, "vt {} {}", uv[0], uv[1])?;
    }
    for n in &mesh.normals {
        writeln!(writer, "vn {} {} {}", n[0], n[1], n[2])?;
    }

    let mut current_material = None;
    for face in &mesh.faces {
        if current_material != Some(face.material_index) {
            current_material = Some(face.material_index);
            let name = mesh
                .material_names
                .get(face.material_index)
                .cloned()
                .unwrap_or_else(|| format!("material_{}", face.material_index));
            writeln!(writer, "usemtl {}", name)?;
        }

        write!(writer, "f")?;
        for corner in &face.corners {
            // OBJ indices are 1-based.
            match (corner.uv, corner.normal) {
                (Some(uv), Some(n)) => write!(writer, " {}/{}/{}", corner.position + 1, uv + 1, n + 1)?,
                (Some(uv), None) => write!(writer, " {}/{}", corner.position + 1, uv + 1)?,
                (None, Some(n)) => write!(writer, " {}//{}", corner.position + 1, n + 1)?,
                (None, None) => write!(writer, " {}", corner.position + 1)?,
            }
        }
        writeln!(writer)?;
    }

    Ok(())
}

pub fn write_obj_file(mesh: &MeshInput, path: &Path) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    write_obj(mesh, &mut file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pseudocooker::{Corner, Face};

    fn triangle_mesh() -> MeshInput {
        MeshInput {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: Vec::new(),
            normals: Vec::new(),
            faces: vec![Face {
                material_index: 0,
                corners: vec![Corner::new(0), Corner::new(1), Corner::new(2)],
            }],
            material_names: vec![],
        }
    }

    #[test]
    fn writes_positions_and_bare_face_indices() {
        let mesh = triangle_mesh();
        let mut buf = Vec::new();
        write_obj(&mesh, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();

        assert!(text.contains("v 0 0 0\n"));
        assert!(text.contains("v 1 0 0\n"));
        assert!(text.contains("v 0 1 0\n"));
        assert!(text.contains("f 1 2 3\n"));
    }

    #[test]
    fn labels_material_groups_by_name_and_only_on_change() {
        let mut mesh = triangle_mesh();
        mesh.material_names = vec!["stone".to_string(), "wood".to_string()];
        mesh.faces = vec![
            Face { material_index: 0, corners: vec![Corner::new(0), Corner::new(1), Corner::new(2)] },
            Face { material_index: 0, corners: vec![Corner::new(0), Corner::new(1), Corner::new(2)] },
            Face { material_index: 1, corners: vec![Corner::new(0), Corner::new(1), Corner::new(2)] },
        ];
        let mut buf = Vec::new();
        write_obj(&mesh, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();

        assert_eq!(text.matches("usemtl stone\n").count(), 1);
        assert_eq!(text.matches("usemtl wood\n").count(), 1);
    }

    #[test]
    fn writes_uv_and_normal_indices_when_present() {
        let mut mesh = triangle_mesh();
        mesh.uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        mesh.normals = vec![[0.0, 0.0, 1.0]];
        mesh.faces = vec![Face {
            material_index: 0,
            corners: vec![
                Corner::new(0).with_uv(0).with_normal(0),
                Corner::new(1).with_uv(1).with_normal(0),
                Corner::new(2).with_uv(2).with_normal(0),
            ],
        }];

        let mut buf = Vec::new();
        write_obj(&mesh, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();

        assert!(text.contains("vt 0 0\n"));
        assert!(text.contains("vn 0 0 1\n"));
        assert!(text.contains("f 1/1/1 2/2/1 3/3/1\n"));
    }
}
