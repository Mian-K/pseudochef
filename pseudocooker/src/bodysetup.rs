//! BodySetup export writer
//!
//! Notes:
//!   - Simple collision (AggGeom) omitted, complex collision assumed (CTF_UseComplexAsSimple)

use crate::core::{
    write_byte_enum_property, write_fname_ref, write_name_property, write_none_terminator, Name, NameTable, Writer,
};
use crate::physxpc::{self, Vec3};

const BULKDATA_SINGLE_USE: u32 = 1 << 3;
const BULKDATA_FORCE_INLINE_PAYLOAD: u32 = 1 << 6;

/// A patch to apply once the containing export's absolute SerialOffset in the .uexp is known: (byte
/// position of the placeholder Offset field, byte position where the payload itself starts), both
/// relative to the start of this export's body.
pub type Patch = (usize, usize);

/// FBulkData / FBulkMetaResource, common (non-64-bit) inline form : Flags(4) + ElementCount(4) +
/// SizeOnDisk(4) + Offset(8), then `payload` bytes immediately follow. Offset is informational only
/// for inline payloads (not used on load) but we still compute a real value via the patch
/// mechanism, matching what a real cook writes.
pub fn write_bulk_data_inline(w: &mut Writer, flags: u32, payload: &[u8], patches: &mut Vec<Patch>) {
    w.u32(flags);
    w.i32(payload.len() as i32);
    w.i32(payload.len() as i32);
    let offset_pos = w.tell();
    w.i64(0); // placeholder, patched once this export's absolute SerialOffset is known
    let payload_pos = w.tell();
    w.bytes(payload);
    patches.push((offset_pos, payload_pos));
}

pub fn write_format_container_single(
    w: &mut Writer,
    table: &mut NameTable,
    format_name: &str,
    payload: &[u8],
    patches: &mut Vec<Patch>,
) {
    w.i32(1); // NumFormats
    write_fname_ref(w, table, format_name);
    write_bulk_data_inline(w, BULKDATA_SINGLE_USE | BULKDATA_FORCE_INLINE_PAYLOAD, payload, patches);
}

/// Returns (body_bytes, patches).
pub fn write_body_setup(
    table: &mut NameTable,
    body_setup_guid_hex32: &str,
    positions: &[Vec3],
    triangles: &[(u32, u32, u32)],
    material_per_triangle: &[u16],
) -> (Vec<u8>, Vec<Patch>) {
    let mut w = Writer::new();

    // --- tagged properties ---
    // DefaultInstance (StructProperty, StructName=BodyInstance) -- nested list:
    //   ObjectType (ByteProperty+EnumName=ECollisionChannel, value=FName)
    //   CollisionProfileName (NameProperty)
    let mut inner = Writer::new();
    write_byte_enum_property(&mut inner, table, "ObjectType", "ECollisionChannel", "ECC_WorldStatic");
    write_name_property(&mut inner, table, "CollisionProfileName", Name::from("BlockAll"));
    write_none_terminator(&mut inner, table);
    crate::core::write_struct_property(&mut w, table, "DefaultInstance", "BodyInstance", &inner.getvalue());

    write_byte_enum_property(&mut w, table, "CollisionTraceFlag", "ECollisionTraceFlag", "CTF_UseComplexAsSimple");
    write_none_terminator(&mut w, table);

    // --- hidden lazy-object-guid presence ---
    w.ubool(false);

    // --- native tail ---
    w.fguid(body_setup_guid_hex32);
    w.ubool(true); // bCooked
    w.ubool(true); // bHasCookedCollisionData

    let physxpc_blob = physxpc::encode(positions, triangles, Some(material_per_triangle));
    let mut patches = Vec::new();
    write_format_container_single(&mut w, table, "PhysXPC", &physxpc_blob, &mut patches);

    (w.into_bytes(), patches)
}
