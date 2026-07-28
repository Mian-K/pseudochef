//! Navmesh collision (deliberately omitted; flesh out later if needed)

use crate::core::{write_bool_property, write_none_terminator, NameTable, Writer};

const MAGIC_NUM: u32 = 0xA237F237;
const VERSION_SHAPE_GEO_EXPORT: i32 = 4;

pub fn write_nav_collision(table: &mut NameTable) -> Vec<u8> {
    let mut w = Writer::new();

    write_bool_property(&mut w, table, "bGatherConvexGeometry", false);
    write_none_terminator(&mut w, table);

    w.ubool(false); // hidden lazy-object-guid presence

    w.u32(MAGIC_NUM);
    w.i32(VERSION_SHAPE_GEO_EXPORT);
    w.fguid(&"0".repeat(32)); // dummy guid, unused/legacy
    w.ubool(true); // bCooked

    // bProcessCookedData = false (bGatherConvexGeometry=false, no Box/Cylinder
    // collision) -> CookedFormatData block is entirely absent.

    w.i32(0); // AreaClass (FPackageIndex, null)

    w.into_bytes()
}
