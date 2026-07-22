//! Core low-level infrastructure shared by the cooked-UE5.1-package encoder:
//! byte writer, FName table, FPropertyTag (tagged-property) writer.
//!
//! Direct port of `cook_core.py`, byte-for-byte. See that file (and
//! `/workspace/README.txt` / the UE5.1 source it was reverse-engineered
//! against) for the authoritative format citations; this module just
//! implements the same write-side rules in Rust.

pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn tell(&self) -> usize {
        self.buf.len()
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// section 0: bool is ALWAYS 4 bytes on disk (legacy UBOOL)
    pub fn ubool(&mut self, v: bool) {
        self.i32(if v { 1 } else { 0 });
    }

    pub fn fstring(&mut self, s: &str) {
        if s.is_empty() {
            self.i32(0);
            return;
        }
        let mut raw = s.as_bytes().to_vec();
        raw.push(0);
        self.i32(raw.len() as i32);
        self.bytes(&raw);
    }

    /// hex32: 32 hex chars (A,B,C,D as 4x uint32, matching the parser's fguid())
    pub fn fguid(&mut self, hex32: &str) {
        assert_eq!(hex32.len(), 32);
        for i in 0..4 {
            let chunk = &hex32[i * 8..i * 8 + 8];
            let v = u32::from_str_radix(chunk, 16).expect("valid hex32 guid");
            self.u32(v);
        }
    }

    pub fn getvalue(&self) -> Vec<u8> {
        self.buf.clone()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

/// FName interning table -- names are appended in first-use order, exactly
/// matching what a cooker would do (section 2: "opaque ordered string
/// table", one entry per unique FName string).
pub struct NameTable {
    pub names: Vec<String>,
    index: std::collections::HashMap<String, i32>,
}

impl NameTable {
    pub fn new() -> Self {
        NameTable { names: Vec::new(), index: std::collections::HashMap::new() }
    }

    pub fn intern(&mut self, s: &str) -> i32 {
        if let Some(&idx) = self.index.get(s) {
            return idx;
        }
        let idx = self.names.len() as i32;
        self.names.push(s.to_string());
        self.index.insert(s.to_string(), idx);
        idx
    }
}

impl Default for NameTable {
    fn default() -> Self {
        Self::new()
    }
}

/// A name reference: base string + instance number. Mirrors Python's
/// `_split()` helper, which accepts either a plain string (number=0) or an
/// explicit (base, number) tuple -- here that's just `"Foo".into()` vs.
/// `("Foo", 1).into()`.
#[derive(Clone)]
pub struct Name {
    pub base: String,
    pub number: i32,
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Name { base: s.to_string(), number: 0 }
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Name { base: s, number: 0 }
    }
}

impl From<&String> for Name {
    fn from(s: &String) -> Self {
        Name { base: s.clone(), number: 0 }
    }
}

impl From<(&str, i32)> for Name {
    fn from((s, n): (&str, i32)) -> Self {
        Name { base: s.to_string(), number: n }
    }
}

impl From<(String, i32)> for Name {
    fn from((s, n): (String, i32)) -> Self {
        Name { base: s, number: n }
    }
}

pub const NONE_NAME: &str = "None";

/// FName REFERENCE (section 0): int32 NameIndex + int32 Number, always 8 bytes.
pub fn write_fname_ref(w: &mut Writer, table: &mut NameTable, name: impl Into<Name>) {
    let name = name.into();
    w.i32(table.intern(&name.base));
    w.i32(name.number);
}

/// FName TABLE ENTRY (section 2): FString + 2x uint16 hash (unused on load,
/// but must be present; we write 0 since nothing checks these on modern load).
pub fn write_name_table_entry(w: &mut Writer, name: &str) {
    w.fstring(name);
    w.u16(0);
    w.u16(0);
}

pub fn write_none_terminator(w: &mut Writer, table: &mut NameTable) {
    write_fname_ref(w, table, NONE_NAME);
}

// ---------------------------------------------------------------------------
// FPropertyTag (tagged property) writer -- section 6
// ---------------------------------------------------------------------------

pub const ZERO_GUID: &str = "00000000000000000000000000000000";

/// Extra, property-type-dependent fields that follow the common tag header.
/// Mirrors write_tag_header's keyword arguments.
#[derive(Default)]
pub struct TagExtra<'a> {
    pub struct_name: Option<&'a str>,
    pub struct_guid: Option<&'a str>,
    pub enum_name: Option<&'a str>,
    pub inner_type: Option<&'a str>,
    pub value_type: Option<&'a str>,
}

pub fn write_tag_header(
    w: &mut Writer,
    table: &mut NameTable,
    name: impl Into<Name>,
    ptype: &str,
    size: i32,
    array_index: i32,
    extra: TagExtra,
) {
    write_fname_ref(w, table, name);
    write_fname_ref(w, table, ptype);
    w.i32(size);
    w.i32(array_index);
    match ptype {
        "StructProperty" => {
            write_fname_ref(w, table, extra.struct_name.expect("struct_name"));
            let guid = &extra.struct_guid.unwrap_or(&ZERO_GUID[..32]);
            w.fguid(guid);
        }
        "ByteProperty" | "EnumProperty" => {
            write_fname_ref(w, table, extra.enum_name.expect("enum_name"));
        }
        "ArrayProperty" | "SetProperty" => {
            write_fname_ref(w, table, extra.inner_type.expect("inner_type"));
        }
        "MapProperty" => {
            write_fname_ref(w, table, extra.inner_type.expect("inner_type"));
            write_fname_ref(w, table, extra.value_type.expect("value_type"));
        }
        _ => {}
    }
}

pub fn write_bool_property(w: &mut Writer, table: &mut NameTable, name: impl Into<Name>, value: bool) {
    write_fname_ref(w, table, name);
    write_fname_ref(w, table, "BoolProperty");
    w.i32(0); // Size always 0 for bools
    w.i32(0); // ArrayIndex
    w.u8(if value { 1 } else { 0 }); // BoolVal, inlined in the tag
    w.u8(0); // HasPropertyGuid
}

pub fn write_value_property(
    w: &mut Writer,
    table: &mut NameTable,
    name: impl Into<Name>,
    ptype: &str,
    value_bytes: &[u8],
    extra: TagExtra,
) {
    write_tag_header(w, table, name, ptype, value_bytes.len() as i32, 0, extra);
    w.u8(0); // HasPropertyGuid
    w.bytes(value_bytes);
}

pub fn write_struct_property(
    w: &mut Writer,
    table: &mut NameTable,
    name: impl Into<Name>,
    struct_name: &str,
    value_bytes: &[u8],
) {
    write_tag_header(
        w,
        table,
        name,
        "StructProperty",
        value_bytes.len() as i32,
        0,
        TagExtra { struct_name: Some(struct_name), ..Default::default() },
    );
    w.u8(0);
    w.bytes(value_bytes);
}

/// ByteProperty with a non-'None' EnumName: value is an FName reference (Size=8).
pub fn write_byte_enum_property(
    w: &mut Writer,
    table: &mut NameTable,
    name: impl Into<Name>,
    enum_name: &str,
    enum_value_name: &str,
) {
    let mut value_w = Writer::new();
    write_fname_ref(&mut value_w, table, enum_value_name);
    let value_bytes = value_w.getvalue();
    write_tag_header(
        w,
        table,
        name,
        "ByteProperty",
        value_bytes.len() as i32,
        0,
        TagExtra { enum_name: Some(enum_name), ..Default::default() },
    );
    w.u8(0);
    w.bytes(&value_bytes);
}

pub fn write_name_property(w: &mut Writer, table: &mut NameTable, name: impl Into<Name>, value_name: impl Into<Name>) {
    let mut value_w = Writer::new();
    write_fname_ref(&mut value_w, table, value_name);
    write_value_property(w, table, name, "NameProperty", &value_w.getvalue(), TagExtra::default());
}

pub fn write_float_property(w: &mut Writer, table: &mut NameTable, name: impl Into<Name>, value: f32) {
    let mut value_w = Writer::new();
    value_w.f32(value);
    write_value_property(w, table, name, "FloatProperty", &value_w.getvalue(), TagExtra::default());
}

pub fn write_double_property(w: &mut Writer, table: &mut NameTable, name: impl Into<Name>, value: f64) {
    let mut value_w = Writer::new();
    value_w.f64(value);
    write_value_property(w, table, name, "DoubleProperty", &value_w.getvalue(), TagExtra::default());
}

/// StructProperty, StructName=Vector -- FVector IS a native/`immutable`
/// USTRUCT (NoExportTypes.h:510 `USTRUCT(immutable, ...) struct FVector`),
/// so unlike FBoxSphereBounds this one genuinely is flat/intrinsic when tagged.
pub fn write_vector_property(w: &mut Writer, table: &mut NameTable, name: impl Into<Name>, x: f64, y: f64, z: f64) {
    write_struct_property(w, table, name, "Vector", &encode_vector(x, y, z));
}

/// StructProperty, StructName=BoxSphereBounds.
///
/// IMPORTANT: unlike FVector/FGuid (both declared `USTRUCT(immutable, ...)`
/// in NoExportTypes.h, which get flat/native tagged-property serialization
/// via UScriptStruct::UseBinarySerialization() due to STRUCT_Immutable),
/// FBoxSphereBounds's USTRUCT declaration has NO immutable specifier
/// (NoExportTypes.h:1413-1414) -- so as a TAGGED property it goes through
/// the GENERIC RECURSIVE SerializeTaggedProperties path: Origin and
/// BoxExtent as their own nested Vector StructProperty tags, SphereRadius
/// as a nested DoubleProperty tag, terminated by None.
///
/// (The native, non-tagged "Bounds" field on FStaticMeshRenderData is a
/// DIFFERENT code path -- a hand-written `Ar << Bounds;` native struct
/// serialize, which genuinely IS flat/56-byte; don't use this function for
/// that field. See encode_box_sphere_bounds() instead.)
pub fn write_box_sphere_bounds_property(
    w: &mut Writer,
    table: &mut NameTable,
    name: impl Into<Name>,
    origin: [f64; 3],
    extent: [f64; 3],
    radius: f64,
) {
    let mut inner = Writer::new();
    write_vector_property(&mut inner, table, "Origin", origin[0], origin[1], origin[2]);
    write_vector_property(&mut inner, table, "BoxExtent", extent[0], extent[1], extent[2]);
    write_double_property(&mut inner, table, "SphereRadius", radius);
    write_none_terminator(&mut inner, table);
    write_struct_property(w, table, name, "BoxSphereBounds", &inner.getvalue());
}

// --- intrinsic struct value encoders (write just the VALUE bytes; caller
//     wraps with write_struct_property) ---

pub fn encode_vector(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut w = Writer::new();
    w.f64(x);
    w.f64(y);
    w.f64(z);
    w.into_bytes()
}

pub fn encode_box_sphere_bounds(origin: [f64; 3], extent: [f64; 3], radius: f64) -> Vec<u8> {
    let mut w = Writer::new();
    w.f64(origin[0]);
    w.f64(origin[1]);
    w.f64(origin[2]);
    w.f64(extent[0]);
    w.f64(extent[1]);
    w.f64(extent[2]);
    w.f64(radius);
    w.into_bytes()
}
