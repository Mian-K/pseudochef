//! NavCollision export writer -- inverse of section 9.4.
//!
//! SCOPE DECISION: rather than reverse-engineering the separate
//! "NavCollision_Chaos" opaque blob format (a second, independent Chaos
//! convex-geometry cooking path used only for navmesh generation -- distinct
//! from BodySetup's PhysXPC complex-collision blob, which IS fully
//! decoded/encoded), this writer explicitly sets `bGatherConvexGeometry=false`
//! as a non-default tagged property. Per section 9.4:
//!
//!     bProcessCookedData = Version>=4 ? (bGatherConvexGeometry ||
//!         BoxCollision.Num()>0 || CylinderCollision.Num()>0) : ...
//!
//! With bGatherConvexGeometry=false and no BoxCollision/CylinderCollision
//! entries, bProcessCookedData evaluates false, so the CookedFormatData block
//! is entirely ABSENT -- no need to know its byte format. This is a real,
//! documented UPROPERTY (not a hack): it's the same toggle a level designer
//! would flip in the editor to say "don't auto-generate nav collision from
//! this mesh's geometry". The practical effect is that this mesh won't
//! contribute navmesh-shaped convex hulls from its render geometry (gameplay
//! physics collision via BodySetup/PhysXPC is unaffected).

use crate::core::{write_bool_property, write_none_terminator, NameTable, Writer};

const MAGIC_NUM: u32 = 0xA237F237;
const VERSION_SHAPE_GEO_EXPORT: i32 = 4;

pub fn write_nav_collision(table: &mut NameTable) -> Vec<u8> {
    let mut w = Writer::new();

    write_bool_property(&mut w, table, "bGatherConvexGeometry", false);
    write_none_terminator(&mut w, table);

    w.ubool(false); // hidden lazy-object-guid presence bool (section 7)

    w.u32(MAGIC_NUM);
    w.i32(VERSION_SHAPE_GEO_EXPORT);
    w.fguid(&"0".repeat(32)); // dummy guid, unused/legacy
    w.ubool(true); // bCooked

    // bProcessCookedData = false (bGatherConvexGeometry=false, no Box/Cylinder
    // collision) -> CookedFormatData block is entirely absent.

    w.i32(0); // AreaClass (FPackageIndex, null)

    w.into_bytes()
}
