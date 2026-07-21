//! A small, stable, self-describing binary codec for the `protoc-rs-schema`
//! descriptor types.
//!
//! The sibling `protoc-rs-schema` types implement only
//! `Debug + Clone + Default + PartialEq` — they are **not** `prost::Message`
//! and do **not** derive `serde`. So we cannot reuse a serialization that the
//! sibling crate provides; this module owns one.
//!
//! The encoding is deliberately simple and version-prefixed (see `blob.rs`):
//!
//! * unsigned integers are LEB128 varints,
//! * signed integers are zig-zag + varint,
//! * `Option<T>` is a one-byte presence tag (0/1) followed by `T` when present,
//! * `Vec<T>` is a varint length followed by that many `T`,
//! * `String` / `Vec<u8>` is a varint length followed by the raw bytes,
//! * enums are encoded by their numeric discriminant.
//!
//! Round-trip identity (`decode(encode(x)) == x`) is what the headline test in
//! `lib.rs` relies on, so every descriptor field is preserved here — except
//! `source_span`, which is `#[doc(hidden)]`, parser-only, and explicitly "not
//! serialized" per the upstream descriptor docs.

use protoc_rs_schema::*;

// ─────────────────────────────── Writer ────────────────────────────────────

/// A append-only binary writer.
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_uvarint(&mut self, mut v: u64) {
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            self.buf.push(byte);
            if v == 0 {
                break;
            }
        }
    }

    pub fn write_ivarint(&mut self, v: i64) {
        // zig-zag
        let zz = ((v << 1) ^ (v >> 63)) as u64;
        self.write_uvarint(zz);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_u8(v as u8);
    }

    pub fn write_str(&mut self, s: &str) {
        self.write_uvarint(s.len() as u64);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn write_bytes(&mut self, b: &[u8]) {
        self.write_uvarint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    pub fn write_opt_str(&mut self, s: &Option<String>) {
        match s {
            Some(v) => {
                self.write_u8(1);
                self.write_str(v);
            }
            None => self.write_u8(0),
        }
    }

    pub fn write_opt_bool(&mut self, v: &Option<bool>) {
        match v {
            Some(b) => {
                self.write_u8(1);
                self.write_bool(*b);
            }
            None => self.write_u8(0),
        }
    }

    pub fn write_opt_i32(&mut self, v: &Option<i32>) {
        match v {
            Some(n) => {
                self.write_u8(1);
                self.write_ivarint(*n as i64);
            }
            None => self.write_u8(0),
        }
    }

    pub fn write_opt_u64(&mut self, v: &Option<u64>) {
        match v {
            Some(n) => {
                self.write_u8(1);
                self.write_uvarint(*n);
            }
            None => self.write_u8(0),
        }
    }

    pub fn write_opt_i64(&mut self, v: &Option<i64>) {
        match v {
            Some(n) => {
                self.write_u8(1);
                self.write_ivarint(*n);
            }
            None => self.write_u8(0),
        }
    }

    pub fn write_opt_bytes(&mut self, v: &Option<Vec<u8>>) {
        match v {
            Some(b) => {
                self.write_u8(1);
                self.write_bytes(b);
            }
            None => self.write_u8(0),
        }
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────── Reader ────────────────────────────────────

/// Codec failure: the bytes are truncated or otherwise malformed.
#[derive(Debug)]
pub struct CodecError(pub String);

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "codec error: {}", self.0)
    }
}

impl std::error::Error for CodecError {}

pub type CodecResult<T> = Result<T, CodecError>;

/// A cursor over a byte slice.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Whether the cursor consumed the complete input.
    ///
    /// Versioned containers use this to read fields appended in a
    /// backward-compatible way: an older blob ends cleanly before the new
    /// field, while a partially written new field still fails decoding.
    pub fn is_at_end(&self) -> bool {
        self.pos == self.buf.len()
    }

    pub fn read_u8(&mut self) -> CodecResult<u8> {
        if self.pos >= self.buf.len() {
            return Err(CodecError("unexpected end of input".into()));
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        Ok(b)
    }

    pub fn read_uvarint(&mut self) -> CodecResult<u64> {
        let mut result: u64 = 0;
        let mut shift = 0;
        loop {
            let byte = self.read_u8()?;
            if shift >= 64 {
                return Err(CodecError("varint too long".into()));
            }
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        Ok(result)
    }

    pub fn read_ivarint(&mut self) -> CodecResult<i64> {
        let zz = self.read_uvarint()?;
        Ok(((zz >> 1) as i64) ^ -((zz & 1) as i64))
    }

    pub fn read_bool(&mut self) -> CodecResult<bool> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_str(&mut self) -> CodecResult<String> {
        let len = self.read_uvarint()? as usize;
        if self.pos + len > self.buf.len() {
            return Err(CodecError("string length exceeds input".into()));
        }
        let s = std::str::from_utf8(&self.buf[self.pos..self.pos + len])
            .map_err(|e| CodecError(format!("invalid utf-8: {e}")))?
            .to_string();
        self.pos += len;
        Ok(s)
    }

    pub fn read_bytes(&mut self) -> CodecResult<Vec<u8>> {
        let len = self.read_uvarint()? as usize;
        if self.pos + len > self.buf.len() {
            return Err(CodecError("bytes length exceeds input".into()));
        }
        let v = self.buf[self.pos..self.pos + len].to_vec();
        self.pos += len;
        Ok(v)
    }

    pub fn read_opt_str(&mut self) -> CodecResult<Option<String>> {
        if self.read_u8()? == 1 {
            Ok(Some(self.read_str()?))
        } else {
            Ok(None)
        }
    }

    pub fn read_opt_bool(&mut self) -> CodecResult<Option<bool>> {
        if self.read_u8()? == 1 {
            Ok(Some(self.read_bool()?))
        } else {
            Ok(None)
        }
    }

    pub fn read_opt_i32(&mut self) -> CodecResult<Option<i32>> {
        if self.read_u8()? == 1 {
            Ok(Some(self.read_ivarint()? as i32))
        } else {
            Ok(None)
        }
    }

    pub fn read_opt_u64(&mut self) -> CodecResult<Option<u64>> {
        if self.read_u8()? == 1 {
            Ok(Some(self.read_uvarint()?))
        } else {
            Ok(None)
        }
    }

    pub fn read_opt_i64(&mut self) -> CodecResult<Option<i64>> {
        if self.read_u8()? == 1 {
            Ok(Some(self.read_ivarint()?))
        } else {
            Ok(None)
        }
    }

    pub fn read_opt_bytes(&mut self) -> CodecResult<Option<Vec<u8>>> {
        if self.read_u8()? == 1 {
            Ok(Some(self.read_bytes()?))
        } else {
            Ok(None)
        }
    }
}

// ──────────────────────── Encode / Decode trait ────────────────────────────

pub trait Codec: Sized {
    fn encode(&self, w: &mut Writer);
    fn decode(r: &mut Reader) -> CodecResult<Self>;
}

fn write_vec<T: Codec>(w: &mut Writer, items: &[T]) {
    w.write_uvarint(items.len() as u64);
    for it in items {
        it.encode(w);
    }
}

fn read_vec<T: Codec>(r: &mut Reader) -> CodecResult<Vec<T>> {
    let len = r.read_uvarint()? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(T::decode(r)?);
    }
    Ok(out)
}

fn write_opt<T: Codec>(w: &mut Writer, v: &Option<T>) {
    match v {
        Some(x) => {
            w.write_u8(1);
            x.encode(w);
        }
        None => w.write_u8(0),
    }
}

fn read_opt<T: Codec>(r: &mut Reader) -> CodecResult<Option<T>> {
    if r.read_u8()? == 1 {
        Ok(Some(T::decode(r)?))
    } else {
        Ok(None)
    }
}

fn write_str_vec(w: &mut Writer, items: &[String]) {
    w.write_uvarint(items.len() as u64);
    for it in items {
        w.write_str(it);
    }
}

fn read_str_vec(r: &mut Reader) -> CodecResult<Vec<String>> {
    let len = r.read_uvarint()? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(r.read_str()?);
    }
    Ok(out)
}

fn write_i32_vec(w: &mut Writer, items: &[i32]) {
    w.write_uvarint(items.len() as u64);
    for it in items {
        w.write_ivarint(*it as i64);
    }
}

fn read_i32_vec(r: &mut Reader) -> CodecResult<Vec<i32>> {
    let len = r.read_uvarint()? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(r.read_ivarint()? as i32);
    }
    Ok(out)
}

// ─────────────────────────── Enum codecs ───────────────────────────────────

impl Codec for Visibility {
    fn encode(&self, w: &mut Writer) {
        w.write_uvarint(*self as i32 as u64);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        match r.read_uvarint()? {
            0 => Ok(Visibility::Export),
            1 => Ok(Visibility::Local),
            v => Err(CodecError(format!("unknown Visibility: {v}"))),
        }
    }
}

impl Codec for FieldType {
    fn encode(&self, w: &mut Writer) {
        w.write_uvarint(*self as i32 as u64);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        let v = r.read_uvarint()? as i32;
        FieldType::from_int(v).ok_or_else(|| CodecError(format!("unknown FieldType: {v}")))
    }
}

impl Codec for FieldLabel {
    fn encode(&self, w: &mut Writer) {
        w.write_uvarint(*self as i32 as u64);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        let v = r.read_uvarint()? as i32;
        FieldLabel::from_int(v).ok_or_else(|| CodecError(format!("unknown FieldLabel: {v}")))
    }
}

impl Codec for OptimizeMode {
    fn encode(&self, w: &mut Writer) {
        w.write_uvarint(*self as i32 as u64);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        match r.read_uvarint()? {
            1 => Ok(OptimizeMode::Speed),
            2 => Ok(OptimizeMode::CodeSize),
            3 => Ok(OptimizeMode::LiteRuntime),
            v => Err(CodecError(format!("unknown OptimizeMode: {v}"))),
        }
    }
}

impl Codec for FieldCType {
    fn encode(&self, w: &mut Writer) {
        w.write_uvarint(*self as i32 as u64);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        match r.read_uvarint()? {
            0 => Ok(FieldCType::String),
            1 => Ok(FieldCType::Cord),
            2 => Ok(FieldCType::StringPiece),
            v => Err(CodecError(format!("unknown FieldCType: {v}"))),
        }
    }
}

impl Codec for FieldJsType {
    fn encode(&self, w: &mut Writer) {
        w.write_uvarint(*self as i32 as u64);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        match r.read_uvarint()? {
            0 => Ok(FieldJsType::JsNormal),
            1 => Ok(FieldJsType::JsString),
            2 => Ok(FieldJsType::JsNumber),
            v => Err(CodecError(format!("unknown FieldJsType: {v}"))),
        }
    }
}

impl Codec for IdempotencyLevel {
    fn encode(&self, w: &mut Writer) {
        w.write_uvarint(*self as i32 as u64);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        match r.read_uvarint()? {
            0 => Ok(IdempotencyLevel::IdempotencyUnknown),
            1 => Ok(IdempotencyLevel::NoSideEffects),
            2 => Ok(IdempotencyLevel::Idempotent),
            v => Err(CodecError(format!("unknown IdempotencyLevel: {v}"))),
        }
    }
}

// ──────────────────────── Option message codecs ────────────────────────────

impl Codec for NamePart {
    fn encode(&self, w: &mut Writer) {
        w.write_str(&self.name_part);
        w.write_bool(self.is_extension);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(NamePart {
            name_part: r.read_str()?,
            is_extension: r.read_bool()?,
        })
    }
}

impl Codec for UninterpretedOption {
    fn encode(&self, w: &mut Writer) {
        write_vec(w, &self.name);
        w.write_opt_str(&self.identifier_value);
        w.write_opt_u64(&self.positive_int_value);
        w.write_opt_i64(&self.negative_int_value);
        w.write_opt_str(&self.double_value);
        w.write_opt_bytes(&self.string_value);
        w.write_opt_str(&self.aggregate_value);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(UninterpretedOption {
            name: read_vec(r)?,
            identifier_value: r.read_opt_str()?,
            positive_int_value: r.read_opt_u64()?,
            negative_int_value: r.read_opt_i64()?,
            double_value: r.read_opt_str()?,
            string_value: r.read_opt_bytes()?,
            aggregate_value: r.read_opt_str()?,
        })
    }
}

impl Codec for FileOptions {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.java_package);
        w.write_opt_str(&self.java_outer_classname);
        w.write_opt_bool(&self.java_multiple_files);
        w.write_opt_bool(&self.java_generate_equals_and_hash);
        w.write_opt_bool(&self.java_string_check_utf8);
        write_opt(w, &self.optimize_for);
        w.write_opt_str(&self.go_package);
        w.write_opt_bool(&self.cc_generic_services);
        w.write_opt_bool(&self.java_generic_services);
        w.write_opt_bool(&self.py_generic_services);
        w.write_opt_bool(&self.deprecated);
        w.write_opt_bool(&self.cc_enable_arenas);
        w.write_opt_str(&self.objc_class_prefix);
        w.write_opt_str(&self.csharp_namespace);
        w.write_opt_str(&self.swift_prefix);
        w.write_opt_str(&self.php_class_prefix);
        w.write_opt_str(&self.php_namespace);
        w.write_opt_str(&self.php_metadata_namespace);
        w.write_opt_str(&self.ruby_package);
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(FileOptions {
            java_package: r.read_opt_str()?,
            java_outer_classname: r.read_opt_str()?,
            java_multiple_files: r.read_opt_bool()?,
            java_generate_equals_and_hash: r.read_opt_bool()?,
            java_string_check_utf8: r.read_opt_bool()?,
            optimize_for: read_opt(r)?,
            go_package: r.read_opt_str()?,
            cc_generic_services: r.read_opt_bool()?,
            java_generic_services: r.read_opt_bool()?,
            py_generic_services: r.read_opt_bool()?,
            deprecated: r.read_opt_bool()?,
            cc_enable_arenas: r.read_opt_bool()?,
            objc_class_prefix: r.read_opt_str()?,
            csharp_namespace: r.read_opt_str()?,
            swift_prefix: r.read_opt_str()?,
            php_class_prefix: r.read_opt_str()?,
            php_namespace: r.read_opt_str()?,
            php_metadata_namespace: r.read_opt_str()?,
            ruby_package: r.read_opt_str()?,
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for MessageOptions {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_bool(&self.message_set_wire_format);
        w.write_opt_bool(&self.no_standard_descriptor_accessor);
        w.write_opt_bool(&self.deprecated);
        w.write_opt_bool(&self.map_entry);
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(MessageOptions {
            message_set_wire_format: r.read_opt_bool()?,
            no_standard_descriptor_accessor: r.read_opt_bool()?,
            deprecated: r.read_opt_bool()?,
            map_entry: r.read_opt_bool()?,
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for FieldOptions {
    fn encode(&self, w: &mut Writer) {
        write_opt(w, &self.ctype);
        w.write_opt_bool(&self.packed);
        write_opt(w, &self.jstype);
        w.write_opt_bool(&self.lazy);
        w.write_opt_bool(&self.deprecated);
        w.write_opt_bool(&self.weak);
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(FieldOptions {
            ctype: read_opt(r)?,
            packed: r.read_opt_bool()?,
            jstype: read_opt(r)?,
            lazy: r.read_opt_bool()?,
            deprecated: r.read_opt_bool()?,
            weak: r.read_opt_bool()?,
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for OneofOptions {
    fn encode(&self, w: &mut Writer) {
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(OneofOptions {
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for EnumOptions {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_bool(&self.allow_alias);
        w.write_opt_bool(&self.deprecated);
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(EnumOptions {
            allow_alias: r.read_opt_bool()?,
            deprecated: r.read_opt_bool()?,
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for EnumValueOptions {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_bool(&self.deprecated);
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(EnumValueOptions {
            deprecated: r.read_opt_bool()?,
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for ServiceOptions {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_bool(&self.deprecated);
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(ServiceOptions {
            deprecated: r.read_opt_bool()?,
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for MethodOptions {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_bool(&self.deprecated);
        write_opt(w, &self.idempotency_level);
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(MethodOptions {
            deprecated: r.read_opt_bool()?,
            idempotency_level: read_opt(r)?,
            uninterpreted_option: read_vec(r)?,
        })
    }
}

impl Codec for ExtensionRangeOptions {
    fn encode(&self, w: &mut Writer) {
        write_vec(w, &self.uninterpreted_option);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(ExtensionRangeOptions {
            uninterpreted_option: read_vec(r)?,
        })
    }
}

// ─────────────────────── Structural message codecs ─────────────────────────

impl Codec for ExtensionRange {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_i32(&self.start);
        w.write_opt_i32(&self.end);
        write_opt(w, &self.options);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(ExtensionRange {
            start: r.read_opt_i32()?,
            end: r.read_opt_i32()?,
            options: read_opt(r)?,
        })
    }
}

impl Codec for ReservedRange {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_i32(&self.start);
        w.write_opt_i32(&self.end);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(ReservedRange {
            start: r.read_opt_i32()?,
            end: r.read_opt_i32()?,
        })
    }
}

impl Codec for EnumReservedRange {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_i32(&self.start);
        w.write_opt_i32(&self.end);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(EnumReservedRange {
            start: r.read_opt_i32()?,
            end: r.read_opt_i32()?,
        })
    }
}

impl Codec for OneofDescriptorProto {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.name);
        write_opt(w, &self.options);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(OneofDescriptorProto {
            name: r.read_opt_str()?,
            options: read_opt(r)?,
        })
    }
}

impl Codec for FieldDescriptorProto {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.name);
        w.write_opt_i32(&self.number);
        write_opt(w, &self.label);
        write_opt(w, &self.r#type);
        w.write_opt_str(&self.type_name);
        w.write_opt_str(&self.extendee);
        w.write_opt_str(&self.default_value);
        w.write_opt_i32(&self.oneof_index);
        w.write_opt_str(&self.json_name);
        write_opt(w, &self.options);
        w.write_opt_bool(&self.proto3_optional);
        // source_span is parser-only / not serialized.
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(FieldDescriptorProto {
            name: r.read_opt_str()?,
            number: r.read_opt_i32()?,
            label: read_opt(r)?,
            r#type: read_opt(r)?,
            type_name: r.read_opt_str()?,
            extendee: r.read_opt_str()?,
            default_value: r.read_opt_str()?,
            oneof_index: r.read_opt_i32()?,
            json_name: r.read_opt_str()?,
            options: read_opt(r)?,
            proto3_optional: r.read_opt_bool()?,
            source_span: None,
        })
    }
}

impl Codec for EnumValueDescriptorProto {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.name);
        w.write_opt_i32(&self.number);
        write_opt(w, &self.options);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(EnumValueDescriptorProto {
            name: r.read_opt_str()?,
            number: r.read_opt_i32()?,
            options: read_opt(r)?,
            source_span: None,
        })
    }
}

impl Codec for EnumDescriptorProto {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.name);
        write_vec(w, &self.value);
        write_opt(w, &self.options);
        write_vec(w, &self.reserved_range);
        write_str_vec(w, &self.reserved_name);
        write_opt(w, &self.visibility);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(EnumDescriptorProto {
            name: r.read_opt_str()?,
            value: read_vec(r)?,
            options: read_opt(r)?,
            reserved_range: read_vec(r)?,
            reserved_name: read_str_vec(r)?,
            visibility: read_opt(r)?,
            source_span: None,
        })
    }
}

impl Codec for MethodDescriptorProto {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.name);
        w.write_opt_str(&self.input_type);
        w.write_opt_str(&self.output_type);
        write_opt(w, &self.options);
        w.write_opt_bool(&self.client_streaming);
        w.write_opt_bool(&self.server_streaming);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(MethodDescriptorProto {
            name: r.read_opt_str()?,
            input_type: r.read_opt_str()?,
            output_type: r.read_opt_str()?,
            options: read_opt(r)?,
            client_streaming: r.read_opt_bool()?,
            server_streaming: r.read_opt_bool()?,
        })
    }
}

impl Codec for ServiceDescriptorProto {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.name);
        write_vec(w, &self.method);
        write_opt(w, &self.options);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(ServiceDescriptorProto {
            name: r.read_opt_str()?,
            method: read_vec(r)?,
            options: read_opt(r)?,
        })
    }
}

impl Codec for DescriptorProto {
    fn encode(&self, w: &mut Writer) {
        w.write_opt_str(&self.name);
        write_vec(w, &self.field);
        write_vec(w, &self.extension);
        write_vec(w, &self.nested_type);
        write_vec(w, &self.enum_type);
        write_vec(w, &self.extension_range);
        write_vec(w, &self.oneof_decl);
        write_opt(w, &self.options);
        write_vec(w, &self.reserved_range);
        write_str_vec(w, &self.reserved_name);
        write_opt(w, &self.visibility);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(DescriptorProto {
            name: r.read_opt_str()?,
            field: read_vec(r)?,
            extension: read_vec(r)?,
            nested_type: read_vec(r)?,
            enum_type: read_vec(r)?,
            extension_range: read_vec(r)?,
            oneof_decl: read_vec(r)?,
            options: read_opt(r)?,
            reserved_range: read_vec(r)?,
            reserved_name: read_str_vec(r)?,
            visibility: read_opt(r)?,
            source_span: None,
        })
    }
}

// ───────────────────────── SourceCodeInfo codec ────────────────────────────

impl Codec for SourceLocation {
    fn encode(&self, w: &mut Writer) {
        write_i32_vec(w, &self.path);
        write_i32_vec(w, &self.span);
        w.write_opt_str(&self.leading_comments);
        w.write_opt_str(&self.trailing_comments);
        write_str_vec(w, &self.leading_detached_comments);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(SourceLocation {
            path: read_i32_vec(r)?,
            span: read_i32_vec(r)?,
            leading_comments: r.read_opt_str()?,
            trailing_comments: r.read_opt_str()?,
            leading_detached_comments: read_str_vec(r)?,
        })
    }
}

impl Codec for SourceCodeInfo {
    fn encode(&self, w: &mut Writer) {
        write_vec(w, &self.location);
    }
    fn decode(r: &mut Reader) -> CodecResult<Self> {
        Ok(SourceCodeInfo {
            location: read_vec(r)?,
        })
    }
}
