use crate::{
    DescribedBy, Schema, Trace,
    anonymous_union::ChunkedEnum,
    builder::SchemaBuilder,
    described::SelfDescribed,
    indices::{
        FieldNameListIndex, IsEmpty, MemberIndex, MemberListIndex, SchemaNodeIndex,
        SchemaNodeListIndex,
    },
    schema::SchemaNode,
    trace::{ALL_FIELDS_PRESENT, ReadTraceExt, TraceNode, TraceNodeKind},
};
use serde::{
    Serialize,
    ser::{Error as _, SerializeMap, SerializeSeq, SerializeTuple, Serializer},
};
use std::{cell::Cell, fmt::Debug};

impl<T> Serialize for SelfDescribed<T>
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut builder = SchemaBuilder::new();
        let trace = builder.trace(&self.0).map_err(S::Error::custom)?;
        let schema = builder.build().map_err(S::Error::custom)?;
        (&schema, DescribedBy(trace, &schema)).serialize(serializer)
    }
}

impl<'schema, 'trace> Serialize for DescribedBy<'schema, &'trace Trace> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let tail = Cell::new(&*(self.0).0);
        let cursor = TraceCursor::start(self.1, &tail)?;
        cursor.serialize(serializer)
    }
}

impl<'schema> Serialize for DescribedBy<'schema, Trace> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        DescribedBy(&self.0, self.1).serialize(serializer)
    }
}

#[derive(Copy, Clone)]
struct TraceCursor<'a> {
    schema: &'a Schema,
    node: SchemaNode,
    trace: TraceNode,
    data: &'a [u8],
    tail: &'a Cell<&'a [u8]>,
}

#[derive(Copy, Clone)]
enum CheckResult<'a> {
    Simple,
    Discriminated(usize, usize, TraceCursor<'a>),
}

impl Debug for CheckResult<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Simple => f.debug_struct("Simple").finish(),
            Self::Discriminated(discriminator, num_variants, cursor) => f
                .debug_struct("Discriminated")
                .field("discriminator", &discriminator)
                .field("num_variants", &num_variants)
                .field("node", &cursor.node)
                .finish(),
        }
    }
}

impl<'a> TraceCursor<'a> {
    #[inline]
    fn start<ErrorT>(schema: &'a Schema, tail: &'a Cell<&'a [u8]>) -> Result<Self, ErrorT>
    where
        ErrorT: serde::ser::Error,
    {
        Ok(Self {
            schema,
            node: schema.node(schema.root_index).map_err(ErrorT::custom)?,
            trace: tail.pop_trace_node()?,
            tail,
            data: tail.get(),
        })
    }

    #[inline]
    fn pop_child<ErrorT>(&self, node: SchemaNodeIndex) -> Result<Self, ErrorT>
    where
        ErrorT: serde::ser::Error,
    {
        Ok(Self {
            schema: self.schema,
            node: self.schema.node(node).map_err(ErrorT::custom)?,
            trace: self.tail.pop_trace_node()?,
            data: self.tail.get(),
            tail: self.tail,
        })
    }

    /// Like [`Self::pop_child`], but reads a scalar straight off the trace, skipping the
    /// general trace-schema check.
    #[inline]
    fn pop_element<ErrorT>(&self, node: SchemaNodeIndex) -> Result<Element<'a>, ErrorT>
    where
        ErrorT: serde::ser::Error,
    {
        let node = self.schema.node(node).map_err(ErrorT::custom)?;
        macro_rules! scalars {
            ($($node:ident => $pop:ident,)+) => {
                match node {
                    $(
                        SchemaNode::$node => {
                            self.expect_trace_kind::<ErrorT>(TraceNodeKind::$node)?;
                            return Ok(Element::Scalar(Scalar::$node(self.tail.$pop()?)));
                        }
                    )+
                    _ => {}
                }
            };
        }
        scalars! {
            Bool => pop_bool,
            I8 => pop_i8,
            I16 => pop_i16,
            I32 => pop_i32,
            I64 => pop_i64,
            I128 => pop_i128,
            U8 => pop_u8,
            U16 => pop_u16,
            U32 => pop_u32,
            U64 => pop_u64,
            U128 => pop_u128,
            F32 => pop_f32,
            F64 => pop_f64,
            Char => pop_char,
        }
        Ok(Element::Cursor(Self {
            schema: self.schema,
            node,
            trace: self.tail.pop_trace_node()?,
            data: self.tail.get(),
            tail: self.tail,
        }))
    }

    #[inline]
    fn traced_child<ErrorT>(&self, node: SchemaNodeIndex, trace: TraceNode) -> Result<Self, ErrorT>
    where
        ErrorT: serde::ser::Error,
    {
        Ok(Self {
            schema: self.schema,
            node: self.schema.node(node).map_err(ErrorT::custom)?,
            trace,
            data: self.tail.get(),
            tail: self.tail,
        })
    }

    #[inline]
    fn serialize_inner<S>(&self, serializer: S, inner: SchemaNodeIndex) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.pop_element(inner)?.serialize(serializer)
    }

    #[inline]
    fn serialize_tuple<S>(
        &self,
        serializer: S,
        length: usize,
        node_list: SchemaNodeListIndex,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let node_list = self.schema.node_list(node_list).map_err(S::Error::custom)?;

        let mut serializer = serializer.serialize_tuple(length)?;
        for &node in node_list {
            serializer.serialize_element(&self.pop_element(node)?)?
        }
        serializer.end()
    }

    #[inline]
    fn serialize_map<S>(
        &self,
        serializer: S,
        length: usize,
        key: SchemaNodeIndex,
        value: SchemaNodeIndex,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut serializer = serializer.serialize_map(Some(length))?;
        for _ in 0..length {
            serializer.serialize_key(&self.pop_element(key)?)?;
            serializer.serialize_value(&self.pop_element(value)?)?;
        }
        serializer.end()
    }

    #[inline]
    fn serialize_sequence<S>(
        &self,
        serializer: S,
        length: usize,
        item: SchemaNodeIndex,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let item_node = self.schema.node(item).map_err(S::Error::custom)?;
        let mut serializer = serializer.serialize_seq(Some(length))?;

        // Sequences of scalars: take all the elements off the trace at once and read each one
        // directly instead of going through a cursor.
        macro_rules! scalar_items {
            ($($node:ident: $ty:ty => |$bytes:ident| $value:expr,)+) => {
                match item_node {
                    $(
                        SchemaNode::$node => {
                            const SIZE: usize = 1 + std::mem::size_of::<$ty>();
                            let items = self.tail.pop_slice::<S::Error>(
                                length
                                    .checked_mul(SIZE)
                                    .ok_or_else(|| S::Error::custom("unexpected end of trace"))?,
                            )?;
                            for item in items.chunks_exact(SIZE) {
                                let (&kind, $bytes) = item.split_first().expect("impossible");
                                if kind != u8::from(TraceNodeKind::$node) {
                                    return Err(S::Error::custom("schema-trace mismatch"));
                                }
                                let $bytes: [u8; SIZE - 1] = $bytes.try_into().expect("impossible");
                                serializer.serialize_element(&$value)?;
                            }
                            return serializer.end();
                        }
                    )+
                    _ => {}
                }
            };
        }
        scalar_items! {
            Bool: u8 => |bytes| bytes[0] != 0,
            I8: i8 => |bytes| i8::from_le_bytes(bytes),
            I16: i16 => |bytes| i16::from_le_bytes(bytes),
            I32: i32 => |bytes| i32::from_le_bytes(bytes),
            I64: i64 => |bytes| i64::from_le_bytes(bytes),
            I128: i128 => |bytes| i128::from_le_bytes(bytes),
            U8: u8 => |bytes| bytes[0],
            U16: u16 => |bytes| u16::from_le_bytes(bytes),
            U32: u32 => |bytes| u32::from_le_bytes(bytes),
            U64: u64 => |bytes| u64::from_le_bytes(bytes),
            U128: u128 => |bytes| u128::from_le_bytes(bytes),
            F32: f32 => |bytes| f32::from_le_bytes(bytes),
            F64: f64 => |bytes| f64::from_le_bytes(bytes),
            Char: u32 => |bytes| char::try_from(u32::from_le_bytes(bytes))
                .map_err(|_| S::Error::custom("bad char in trace"))?,
        }

        // Sequences of structs without skippable or bottom-typed fields: every element has the
        // same header, compared as bytes, and serializes its fields in order.
        if let SchemaNode::Struct(name, name_list_index, skip_list, node_list) = item_node
            && skip_list.is_empty()
        {
            let node_list = self.schema.node_list(node_list).map_err(S::Error::custom)?;
            let name_list = self
                .schema
                .field_name_list(name_list_index)
                .map_err(S::Error::custom)?;
            if name_list.len() == node_list.len() && !node_list.iter().any(IsEmpty::is_empty) {
                let mut header = [0; 17];
                header[0] = TraceNodeKind::Struct.into();
                header[1..5].copy_from_slice(&u32::from(name).to_le_bytes());
                header[5..9].copy_from_slice(&u32::from(name_list_index).to_le_bytes());
                header[9..13].copy_from_slice(
                    &u32::try_from(node_list.len())
                        .map_err(|_| S::Error::custom("too many fields"))?
                        .to_le_bytes(),
                );
                header[13..17].copy_from_slice(&ALL_FIELDS_PRESENT.to_le_bytes());
                for _ in 0..length {
                    if self.tail.pop_slice::<S::Error>(header.len())? != header {
                        return Err(S::Error::custom("schema-trace mismatch"));
                    }
                    serializer.serialize_element(&FieldsSerializer {
                        cursor: self,
                        node_list,
                    })?;
                }
                return serializer.end();
            }
        }

        for _ in 0..length {
            serializer.serialize_element(&self.pop_child(item)?)?;
        }
        serializer.end()
    }

    #[inline]
    fn expect_trace_kind<ErrorT>(&self, kind: TraceNodeKind) -> Result<(), ErrorT>
    where
        ErrorT: serde::ser::Error,
    {
        if self.tail.pop_u8::<ErrorT>()? == u8::from(kind) {
            Ok(())
        } else {
            Err(ErrorT::custom("schema-trace mismatch"))
        }
    }

    #[inline]
    fn serialize_struct<S>(
        &self,
        serializer: S,
        name_list: FieldNameListIndex,
        skip_list: MemberListIndex,
        node_list: SchemaNodeListIndex,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let skip_list = self
            .schema
            .member_list(skip_list)
            .map_err(S::Error::custom)?;
        let node_list = self.schema.node_list(node_list).map_err(S::Error::custom)?;
        let name_list = self
            .schema
            .field_name_list(name_list)
            .map_err(S::Error::custom)?;
        let length = self.tail.pop_length_u32()?;
        let presence_offset = self.tail.pop_u32()?;
        if name_list.len() != node_list.len() {
            return Err(S::Error::custom(
                "field name - field type length mismatch in schema",
            ));
        }
        let presence_size = length * std::mem::size_of::<u32>();

        // Without skippable fields every field that isn't bottom-typed (never serialized) is
        // present, in order: no discriminant, and no need to read the presence list.
        if skip_list.is_empty() {
            if node_list.iter().filter(|node| !node.is_empty()).count() != length {
                return Err(S::Error::custom("schema-trace mismatch"));
            }
            let mut serializer = serializer.serialize_tuple(length)?;
            for &node in node_list {
                if !node.is_empty() {
                    serializer.serialize_element(&self.pop_element(node)?)?;
                }
            }
            if presence_offset != ALL_FIELDS_PRESENT {
                // Bottom-typed fields were skipped.
                self.tail.pop_slice(presence_size)?;
            }
            return serializer.end();
        }

        let presence = if presence_offset == ALL_FIELDS_PRESENT {
            if length != node_list.len() {
                return Err(S::Error::custom("schema-trace mismatch"));
            }
            Presence::All(length)
        } else {
            let offset = usize::try_from(presence_offset).expect("usize must be at least 32 bits");
            Presence::Listed(
                self.tail
                    .get()
                    .get(offset..)
                    .and_then(|rest| rest.get(..presence_size))
                    .ok_or_else(|| S::Error::custom("unexpected end of trace"))?,
            )
        };
        let ok = ChunkedEnum::serializable(
            skip_list.len(),
            discriminant_from_presence(skip_list, presence),
            &SkippableStructSerializer {
                cursor: self,
                presence,
                node_list,
            },
        )?
        .serialize(serializer)?;
        if let Presence::Listed(listed) = presence
            && !std::ptr::eq(self.tail.pop_slice(presence_size)?, listed)
        {
            return Err(S::Error::custom("schema-trace mismatch"));
        }
        Ok(ok)
    }

    // Checks whether the trace matches the schema node.
    //
    // Note that this check is shallow, tightly coupled with the logic in
    // `SchemaBuilderNode::unify`. The assumption is that, within a union there is:
    //  * At most one record type (incl. units and newtypes) with a given (name, variant, field_names, length).
    //  * At most one `Some(_)`, `Sequence[_]`, `Map[_, _]`
    #[inline]
    fn check<ErrorT>(&self) -> Result<Option<CheckResult<'a>>, ErrorT>
    where
        ErrorT: serde::ser::Error,
    {
        let matches = match (self.trace, self.node) {
            (TraceNode::Bool, SchemaNode::Bool)
            | (TraceNode::I8, SchemaNode::I8)
            | (TraceNode::I16, SchemaNode::I16)
            | (TraceNode::I32, SchemaNode::I32)
            | (TraceNode::I64, SchemaNode::I64)
            | (TraceNode::I128, SchemaNode::I128)
            | (TraceNode::U8, SchemaNode::U8)
            | (TraceNode::U16, SchemaNode::U16)
            | (TraceNode::U32, SchemaNode::U32)
            | (TraceNode::U64, SchemaNode::U64)
            | (TraceNode::U128, SchemaNode::U128)
            | (TraceNode::F32, SchemaNode::F32)
            | (TraceNode::F64, SchemaNode::F64)
            | (TraceNode::Char, SchemaNode::Char)
            | (TraceNode::String, SchemaNode::String)
            | (TraceNode::Bytes, SchemaNode::Bytes)
            | (TraceNode::None, SchemaNode::OptionNone)
            | (TraceNode::Some, SchemaNode::OptionSome(_))
            | (TraceNode::Unit, SchemaNode::Unit)
            | (TraceNode::Map, SchemaNode::Map(_, _))
            | (TraceNode::Sequence, SchemaNode::Sequence(_)) => true,

            (TraceNode::UnitStruct(trace_name), SchemaNode::UnitStruct(schema_name))
            | (TraceNode::NewtypeStruct(trace_name), SchemaNode::NewtypeStruct(schema_name, _)) => {
                trace_name == schema_name
            }

            (
                TraceNode::UnitVariant(trace_name, trace_variant),
                SchemaNode::UnitVariant(schema_name, schema_variant),
            )
            | (
                TraceNode::NewtypeVariant(trace_name, trace_variant),
                SchemaNode::NewtypeVariant(schema_name, schema_variant, _),
            ) => (trace_name, trace_variant) == (schema_name, schema_variant),

            (TraceNode::Tuple(trace_length), SchemaNode::Tuple(schema_type_list)) => {
                self.matches_length(trace_length, schema_type_list)?
            }
            (
                TraceNode::TupleStruct(trace_length, trace_name),
                SchemaNode::TupleStruct(schema_name, schema_type_list),
            ) => {
                trace_name == schema_name && self.matches_length(trace_length, schema_type_list)?
            }
            (
                TraceNode::TupleVariant(trace_length, trace_name, trace_variant),
                SchemaNode::TupleVariant(schema_name, schema_variant, schema_type_list),
            ) => {
                (trace_name, trace_variant) == (schema_name, schema_variant)
                    && self.matches_length(trace_length, schema_type_list)?
            }

            (
                TraceNode::Struct(trace_name, trace_name_list),
                SchemaNode::Struct(schema_name, schema_name_list, _, _),
            ) => (trace_name, trace_name_list) == (schema_name, schema_name_list),
            (
                TraceNode::StructVariant(trace_name, trace_variant, trace_name_list),
                SchemaNode::StructVariant(schema_name, schema_variant, schema_name_list, _, _),
            ) => {
                (trace_name, trace_variant, trace_name_list)
                    == (schema_name, schema_variant, schema_name_list)
            }

            (trace, SchemaNode::Union(schema_list)) => {
                let variants = self.schema.node_list(schema_list).map_err(ErrorT::custom)?;
                for (discriminant, &node) in variants.iter().enumerate() {
                    let child = self.traced_child(node, trace)?;
                    if child.check()?.is_some() {
                        return Ok(Some(CheckResult::Discriminated(
                            discriminant,
                            variants.len(),
                            child,
                        )));
                    }
                }
                return Ok(None);
            }

            _ => false,
        };

        Ok(matches.then_some(CheckResult::Simple))
    }

    #[inline]
    fn matches_length<ErrorT>(
        &self,
        trace_length: u32,
        schema_type_list: SchemaNodeListIndex,
    ) -> Result<bool, ErrorT>
    where
        ErrorT: serde::ser::Error,
    {
        Ok(
            usize::try_from(trace_length).expect("usize must be at least 32-bits")
                == self
                    .schema
                    .node_list(schema_type_list)
                    .map_err(ErrorT::custom)?
                    .len(),
        )
    }

    #[inline]
    fn finish_serialize<S>(
        &self,
        serializer: S,
        checked: CheckResult<'_>,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let data = self.tail;
        if let CheckResult::Discriminated(discriminant, num_variants, child) = checked {
            assert!(
                discriminant < num_variants,
                "out of bounds discriminant: {discriminant} >= {num_variants}"
            );
            return ChunkedEnum::serializable(
                usize::try_from(usize::BITS - (num_variants - 1).leading_zeros())
                    .expect("usize must be at least 32 bits"),
                u64::try_from(discriminant)
                    .map_err(|_| S::Error::custom("too many discriminants"))?,
                &child,
            )?
            .serialize(serializer);
        }
        match self.node {
            SchemaNode::Bool => serializer.serialize_bool(data.pop_bool()?),
            SchemaNode::I8 => serializer.serialize_i8(data.pop_i8()?),
            SchemaNode::I16 => serializer.serialize_i16(data.pop_i16()?),
            SchemaNode::I32 => serializer.serialize_i32(data.pop_i32()?),
            SchemaNode::I64 => serializer.serialize_i64(data.pop_i64()?),
            SchemaNode::I128 => serializer.serialize_i128(data.pop_i128()?),
            SchemaNode::U8 => serializer.serialize_u8(data.pop_u8()?),
            SchemaNode::U16 => serializer.serialize_u16(data.pop_u16()?),
            SchemaNode::U32 => serializer.serialize_u32(data.pop_u32()?),
            SchemaNode::U64 => serializer.serialize_u64(data.pop_u64()?),
            SchemaNode::U128 => serializer.serialize_u128(data.pop_u128()?),
            SchemaNode::F32 => serializer.serialize_f32(data.pop_f32()?),
            SchemaNode::F64 => serializer.serialize_f64(data.pop_f64()?),
            SchemaNode::Char => serializer.serialize_char(data.pop_char()?),
            SchemaNode::String => serializer.serialize_str(data.pop_str(data.pop_length_u32()?)?),
            SchemaNode::Bytes => {
                serializer.serialize_bytes(data.pop_slice(data.pop_length_u32()?)?)
            }

            SchemaNode::Unit
            | SchemaNode::UnitStruct(_)
            | SchemaNode::UnitVariant(_, _)
            | SchemaNode::OptionNone => serializer.serialize_unit(),

            SchemaNode::OptionSome(inner)
            | SchemaNode::NewtypeStruct(_, inner)
            | SchemaNode::NewtypeVariant(_, _, inner) => self.serialize_inner(serializer, inner),

            SchemaNode::Map(key, value) => {
                self.serialize_map(serializer, data.pop_length_u32()?, key, value)
            }
            SchemaNode::Sequence(item) => {
                self.serialize_sequence(serializer, data.pop_length_u32()?, item)
            }

            SchemaNode::Tuple(type_list)
            | SchemaNode::TupleStruct(_, type_list)
            | SchemaNode::TupleVariant(_, _, type_list) => self.serialize_tuple(
                serializer,
                self.schema
                    .node_list(type_list)
                    .map_err(S::Error::custom)?
                    .len(),
                type_list,
            ),

            SchemaNode::Struct(_, name_list, skip_list, type_list)
            | SchemaNode::StructVariant(_, _, name_list, skip_list, type_list) => {
                self.serialize_struct(serializer, name_list, skip_list, type_list)
            }

            SchemaNode::Union(_) => unreachable!("union finish called with simple check result"),
        }
    }
}

/// The fields of a struct without skippable or bottom-typed fields, as a tuple.
struct FieldsSerializer<'a, 'v> {
    cursor: &'v TraceCursor<'a>,
    node_list: &'a [SchemaNodeIndex],
}

impl Serialize for FieldsSerializer<'_, '_> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut serializer = serializer.serialize_tuple(self.node_list.len())?;
        for &node in self.node_list {
            serializer.serialize_element(&self.cursor.pop_element(node)?)?;
        }
        serializer.end()
    }
}

/// A value in a sequence, tuple, struct, map or option: a scalar read off the trace, or a
/// cursor for anything else.
enum Element<'a> {
    Scalar(Scalar),
    Cursor(TraceCursor<'a>),
}

impl Serialize for Element<'_> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Element::Scalar(scalar) => scalar.serialize(serializer),
            Element::Cursor(cursor) => cursor.serialize(serializer),
        }
    }
}

#[derive(Copy, Clone)]
enum Scalar {
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    I128(i128),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
    F32(f32),
    F64(f64),
    Char(char),
}

impl Serialize for Scalar {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Scalar::Bool(value) => serializer.serialize_bool(value),
            Scalar::I8(value) => serializer.serialize_i8(value),
            Scalar::I16(value) => serializer.serialize_i16(value),
            Scalar::I32(value) => serializer.serialize_i32(value),
            Scalar::I64(value) => serializer.serialize_i64(value),
            Scalar::I128(value) => serializer.serialize_i128(value),
            Scalar::U8(value) => serializer.serialize_u8(value),
            Scalar::U16(value) => serializer.serialize_u16(value),
            Scalar::U32(value) => serializer.serialize_u32(value),
            Scalar::U64(value) => serializer.serialize_u64(value),
            Scalar::U128(value) => serializer.serialize_u128(value),
            Scalar::F32(value) => serializer.serialize_f32(value),
            Scalar::F64(value) => serializer.serialize_f64(value),
            Scalar::Char(value) => serializer.serialize_char(value),
        }
    }
}

/// The serialized fields of a struct that has skippable fields.
#[derive(Copy, Clone)]
enum Presence<'a> {
    /// No field was skipped: all of this many.
    All(usize),
    /// The indices of the serialized fields, from the trace.
    Listed(&'a [u8]),
}

impl<'a> Presence<'a> {
    #[inline]
    fn len(self) -> usize {
        match self {
            Presence::All(length) => length,
            Presence::Listed(listed) => listed.len() / std::mem::size_of::<u32>(),
        }
    }

    #[inline]
    fn fields(self) -> impl DoubleEndedIterator<Item = MemberIndex> + 'a {
        let (all, listed) = match self {
            Presence::All(length) => (0..length, &[][..]),
            Presence::Listed(listed) => (0..0, listed),
        };
        all.map(|field| MemberIndex::from(u32::try_from(field).expect("field index fits u32")))
            .chain(iter_field_indices(listed))
    }
}

struct SkippableStructSerializer<'a, 'v> {
    cursor: &'v TraceCursor<'a>,
    presence: Presence<'a>,
    node_list: &'a [SchemaNodeIndex],
}

impl<'a, 'v> Serialize for SkippableStructSerializer<'a, 'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut serializer = serializer.serialize_tuple(self.presence.len())?;
        for field in self.presence.fields() {
            serializer.serialize_element(&self.cursor.pop_element(
                *self.node_list.get(usize::from(field)).ok_or_else(|| {
                    S::Error::custom("member index out of bounds for struct in schema")
                })?,
            )?)?;
        }
        serializer.end()
    }
}

fn discriminant_from_presence(skip_list: &[MemberIndex], presence: Presence<'_>) -> u64 {
    let mut discriminant = 0u64;
    let mut presence = presence.fields().rev().peekable();
    for &skip in skip_list.iter().rev() {
        discriminant <<= 1;
        while let Some(&present) = presence.peek() {
            if present > skip {
                presence.next();
                continue;
            }
            if present == skip {
                discriminant |= 1;
                presence.next();
            }
            break;
        }
    }
    discriminant
}

fn iter_field_indices(presence: &[u8]) -> impl DoubleEndedIterator<Item = MemberIndex> {
    presence
        .as_chunks::<{ std::mem::size_of::<MemberIndex>() }>()
        .0
        .iter()
        .map(|&chunk| MemberIndex::from(u32::from_le_bytes(chunk)))
}

// Any issues caused by a mismatch between the schema and the trace are technically bugs but
// instead of panicking, we still propagate errors as `S::Error::custom`. Traces are not
// serializable as such, so they don't come from an untrusted source.
//
// The `object -> trace` part of serialization handles errors gracefully, since the object may be
// untrusted (and of course deserialization does as well).
//
// It's unclear to me whether this is actually better than panicking.
impl Serialize for TraceCursor<'_> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.tail.set(self.data);
        self.finish_serialize(
            serializer,
            self.check()?
                .ok_or_else(|| S::Error::custom("schema-trace mismatch"))?,
        )
    }
}
