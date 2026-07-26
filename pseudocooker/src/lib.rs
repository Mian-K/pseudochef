//! `pseudocooker` -- turns raw vertex/face/UV mesh data into a cooked UE5.1
//! StaticMesh package (.uasset + .uexp + .ubulk), entirely without Unreal
//! Engine, by directly implementing the on-disk cooked-package format
//! reverse-engineered against UE5.1's own source (`/workspace/unreal`).
//!
//! This is a Rust port of a Python prototype (`cook_static_mesh.py` and its
//! supporting modules), with the OBJ-file front end removed: callers hand
//! in vertex/UV/normal/face data directly via [`MeshInput`] instead of a
//! Wavefront OBJ path.
//!
//! Pipeline, mirroring the original's own path end to end:
//!   MeshInput -> render mesh ([`mesh::build_render_mesh`])
//!       -> BodySetup export incl. real PhysXPC collision ([`bodysetup`],
//!          [`physxpc`])
//!       -> NavCollision export ([`navcollision`])
//!       -> StaticMesh export incl. full FStaticMeshRenderData ([`staticmesh`])
//!       -> package assembly: name/import/export tables + header ([`package`])
//!
//! SCOPE / KNOWN LIMITATIONS (all deliberate, ported over from the original
//! -- see the relevant submodule's docs for the full rationale):
//!   - Nanite, Lumen card data, mesh distance fields, ray tracing geometry:
//!     not generated; written using the engine's own strip-flag mechanism so
//!     the file is still fully valid, just without those features baked in.
//!   - NavCollision's own opaque "NavCollision_Chaos" blob format is not
//!     implemented; bGatherConvexGeometry is explicitly set false so that
//!     block is legitimately absent instead.
//!   - Tangent basis is a simple per-face/per-wedge construction, not
//!     UV-derivative-accurate.
//!   - Only a single LOD is produced.
//!   - This has NOT been confirmed to load inside a running UE5.1 editor --
//!     only validated by byte-for-byte comparison against the Python
//!     original's output (see this workspace's validation harness).

pub mod bodysetup;
pub mod core;
pub mod mesh;
pub mod navcollision;
pub mod package;
pub mod physxpc;
pub mod staticmesh;

pub use mesh::{Bounds, Corner, Face, MeshInput, RenderMesh, Section, Tangent, Vec2, Vec3};
pub use package::CookOptions;

/// The three files a cooked UE5.1 StaticMesh package is made of.
pub struct CookedAsset {
    pub uasset: Vec<u8>,
    pub uexp: Vec<u8>,
    /// Nothing is currently streamed out-of-line into the bulk-data file
    /// (see `staticmesh`'s docs on `bInlined`), so this is always empty --
    /// still produced so callers can write the `.ubulk` a real cooked
    /// package ships alongside its `.uasset`/`.uexp`.
    pub ubulk: Vec<u8>,
}

impl CookedAsset {
    /// Writes `<dir>/<asset_name>.{uasset,uexp,ubulk}`.
    pub fn write_to_dir(&self, dir: &std::path::Path, asset_name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join(format!("{asset_name}.uasset")), &self.uasset)?;
        std::fs::write(dir.join(format!("{asset_name}.uexp")), &self.uexp)?;
        std::fs::write(dir.join(format!("{asset_name}.ubulk")), &self.ubulk)?;
        Ok(())
    }
}

/// Cooks `mesh_input` into a UE5.1-compliant StaticMesh package named
/// `asset_name` (becomes `/Game/<asset_name>` and the top-level export's
/// object name).
///
/// Input is expected to already be in UE's own conventions: left-handed,
/// Z-up axes and centimeter units. Package GUIDs are freshly randomized on
/// every call (matching real cooks).
pub fn cook(mesh_input: &MeshInput, asset_name: &str) -> CookedAsset {
    let render_mesh = mesh::build_render_mesh(mesh_input);
    let (uasset, uexp) = package::cook_package(asset_name, &render_mesh, &CookOptions::default());
    CookedAsset { uasset, uexp, ubulk: Vec::new() }
}
