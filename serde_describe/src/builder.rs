use std::fmt::Debug;

use crate::{
    indices::{
        FieldNameIndex, FieldNameListIndex, IndexIsEmpty, IsEmpty, MemberIndex, MemberListIndex,
        SchemaNodeIndex, SchemaNodeListIndex, TraceIndex, TypeName, TypeNameIndex,
        VariantNameIndex,
    },
    pool::{NonEmptyPool, Pool},
    schema::{Schema, SchemaNode},
    trace::{ALL_FIELDS_PRESENT, Trace, TraceNodeKind},
};
use serde::{
    Serialize,
    ser::{
        SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant, Serializer,
    },
};

/// An in-progress schema built by successive calls to [`SchemaBuilder::trace`].
///
/// For simple use-cases, where the schema gets serialized together with data, you can use
/// [`SelfDescribed`][`crate::SelfDescribed`]. If instead, you're serializing many values with
/// similar schemas, you can use a [`SchemaBuilder`].
///
/// Example
/// -------
/// ```rust
/// use serde::{Serialize, Deserialize, de::DeserializeSeed};
/// use serde_describe::{SchemaBuilder, Schema, Trace, DescribedBy};
///
/// #[derive(Debug, PartialEq, Serialize, Deserialize)]
/// struct Type1 {
///     a: u32,
///     b: Type3,
/// }
///
/// #[derive(Debug, PartialEq, Serialize, Deserialize)]
/// struct Type2 {
///     x: Vec<f32>,
///     y: Type3,
/// }
///
/// #[derive(Debug, PartialEq, Serialize, Deserialize)]
/// struct Type3 {
///     m: u32,
///     n: Option<u32>,
/// }
///
/// // Create some sample data to serialize
/// let original1 = Type1 { a: 10, b: Type3 { m: 100, n: Some(200) }};
/// let original2 = Type1 { a: 20, b: Type3 { m: 500, n: None }};
/// let original3 = Type2 { x: vec![1.0, 2.0], y: Type3 { m: 1, n: Some(2) } };
///
/// // Serialize the three values, then build the schema.
/// let mut builder = SchemaBuilder::new();
/// let trace1: Trace = builder.trace(&original1)?;
/// let trace2 = builder.trace(&original2)?;
/// let trace3 = builder.trace(&original3)?;
/// let schema = builder.build()?;
///
/// // Write different files using the same builder, then serialize the schema to its own file.
/// std::fs::write("file1.type1", &postcard::to_stdvec(&schema.describe_trace(trace1))?)?;
/// std::fs::write("file2.type1", &postcard::to_stdvec(&schema.describe_trace(trace2))?)?;
/// std::fs::write("file3.type2", &postcard::to_stdvec(&schema.describe_trace(trace3))?)?;
/// std::fs::write("schema", &postcard::to_stdvec(&schema)?)?;
///
/// // Load the schema, then read the values back.
/// let schema: Schema = postcard::from_bytes(&std::fs::read("schema")?)?;
/// let DescribedBy(roundtripped_value1, _) = schema.describe_type::<Type1>()
///     .deserialize(&mut postcard::Deserializer::from_bytes(&std::fs::read("file1.type1")?))?;
/// let DescribedBy(roundtripped_value2,_) = schema.describe_type::<Type1>()
///     .deserialize(&mut postcard::Deserializer::from_bytes(&std::fs::read("file2.type1")?))?;
/// let DescribedBy(roundtripped_value3,_) = schema.describe_type::<Type2>()
///     .deserialize(&mut postcard::Deserializer::from_bytes(&std::fs::read("file3.type2")?))?;
///
/// assert_eq!(roundtripped_value1, original1);
/// assert_eq!(roundtripped_value2, original2);
/// assert_eq!(roundtripped_value3, original3);
///
/// # std::fs::remove_file("file1.type1")?;
/// # std::fs::remove_file("file2.type1")?;
/// # std::fs::remove_file("file3.type2")?;
/// # std::fs::remove_file("schema")?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Default, Clone)]
pub struct SchemaBuilder {
    root: SchemaBuilderNode,
    nodes: Pool<SchemaNode, SchemaNodeIndex>,
    node_lists: Pool<Box<[SchemaNodeIndex]>, SchemaNodeListIndex>,
    member_lists: Pool<Box<[MemberIndex]>, MemberListIndex>,
    names: Names,
}

impl SchemaBuilder {
    /// Creates a new, empty [`SchemaBuilder`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Converts a type that supports [`serde::Serialize`] into a [`Trace`] and records its type
    /// into the schema.
    ///
    /// If tracing fails, the types recorded so far may already include parts of the failed value.
    /// The schema stays valid for every successfully traced value, it may just describe more
    /// than they need.
    ///
    /// See the top-level [`SchemaBuilder`] documentation for an example.
    pub fn trace<ValueT>(&mut self, value: &ValueT) -> Result<Trace, TraceError>
    where
        ValueT: Serialize,
    {
        let mut data = Vec::new();
        // Left over by a failed trace.
        self.names.skipped.clear();
        value.serialize(RootSerializer {
            data: &mut data,
            names: &mut self.names,
            slot: &mut self.root,
        })?;
        Ok(Trace(data))
    }

    /// Converts all the recorded value types into a schema that can be used to serialize the
    /// [`Trace`]-s returned by [`trace`][`Self::trace`].
    ///
    /// See the top-level [`SchemaBuilder`] documentation for an example.
    pub fn build(mut self) -> Result<Schema, TraceError> {
        let schema = Schema {
            root_index: std::mem::take(&mut self.root).build(&mut self)?,
            nodes: self.nodes.into(),
            node_lists: self.node_lists.into(),
            member_lists: self.member_lists.into(),
            field_name_lists: self.names.field_name_lists.into(),
            field_names: self.names.field_names.into(),
            variant_names: self.names.variant_names.into(),
            type_names: self.names.type_names.into(),
            seq_memo: Default::default(),
            fixed_shape_memo: Default::default(),
        };
        Ok(schema)
    }
}

/// The name pools filled while tracing.
#[derive(Default, Clone)]
pub(crate) struct Names {
    field_name_lists: NonEmptyPool<Box<[FieldNameIndex]>, FieldNameListIndex>,
    field_names: StaticNames<FieldNameIndex>,
    variant_names: StaticNames<VariantNameIndex>,
    type_names: StaticNames<TypeNameIndex>,
    /// The fields skipped by the structs being traced, a stack of one run per struct.
    skipped: Vec<MemberIndex>,
}

/// A pool of `&'static str` names.
type StaticNames<IndexT> = NonEmptyPool<&'static str, IndexT>;

/// Errors returned by tracing values.
#[derive(Debug)]
#[non_exhaustive]
pub enum TraceError {
    /// The value is in some way too large, and built-in limits were exceeded.
    Limit(TraceLimitError),

    /// A `Serialize` implementation serialized a different number of struct fields or tuple
    /// elements than the length it declared.
    LengthMismatch,

    /// Custom serde serialization error.
    Custom(Box<str>),
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tracing error: ")?;
        match self {
            Self::Limit(limit) => write!(f, "{limit}"),
            Self::LengthMismatch => {
                write!(f, "serialized a different number of elements than declared")
            }
            Self::Custom(custom) => write!(f, "custom serialization error: {custom}"),
        }
    }
}

impl From<TraceLimitErrorKind> for TraceError {
    #[inline]
    fn from(kind: TraceLimitErrorKind) -> Self {
        Self::Limit(kind.into())
    }
}

impl std::error::Error for TraceError {
    #[inline]
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Limit(limit) => Some(limit),
            Self::LengthMismatch | Self::Custom(_) => None,
        }
    }
}

/// Errors caused by tracing a value that is in some way too large.
#[derive(Debug)]
pub struct TraceLimitError(TraceLimitErrorKind);

impl From<TraceLimitErrorKind> for TraceLimitError {
    #[inline]
    fn from(value: TraceLimitErrorKind) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for TraceLimitError {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for TraceLimitError {
    #[inline]
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl serde::ser::Error for TraceError {
    #[inline]
    fn custom<T>(msg: T) -> Self
    where
        T: std::fmt::Display,
    {
        TraceError::Custom(msg.to_string().into())
    }
}

pub(crate) const MAX_SKIPPABLE_FIELDS: usize = 64;

#[derive(Debug)]
pub(crate) enum TraceLimitErrorKind {
    SchemaNodes,
    SchemaNodeLists,
    Members,
    MemberLists,
    Names,
    FieldNameLists,
    Values,
    UnionVariants,
    SkippableFields,
}

impl std::fmt::Display for TraceLimitErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::SchemaNodes => "too many schema nodes for u32",
            Self::SchemaNodeLists => "too many schema node lists for u32",
            Self::Members => "too many struct members for u32",
            Self::MemberLists => "too many struct member lists for u32",
            Self::Names => "too many struct/variant/field names for u32",
            Self::FieldNameLists => "too many field lists for u32",
            Self::Values => "too many values for u32",
            Self::UnionVariants => "too many variants",
            Self::SkippableFields => "too many skippable fields",
        };
        f.write_str(message)
    }
}

impl std::error::Error for TraceLimitErrorKind {}

/// Traces one value: appends it to the trace and merges its type into `slot` in place, so values
/// whose type is already recorded (e.g. every element of a sequence after the first) allocate
/// nothing for the schema.
pub(crate) struct RootSerializer<'a> {
    data: &'a mut Vec<u8>,
    names: &'a mut Names,
    slot: &'a mut SchemaBuilderNode,
}

impl RootSerializer<'_> {
    #[inline]
    fn push_struct_name(&mut self, name: &'static str) -> Result<TypeName, TraceLimitErrorKind> {
        let name = self.names.type_names.intern(name)?;
        push_u32(self.data, name.into());
        Ok(TypeName(name, None))
    }

    #[inline]
    fn push_variant_name(
        &mut self,
        name: &'static str,
        variant: &'static str,
    ) -> Result<TypeName, TraceLimitErrorKind> {
        let name = self.names.type_names.intern(name)?;
        let variant = self.names.variant_names.intern(variant)?;
        push_u32(self.data, name.into());
        push_u32(self.data, variant.into());
        Ok(TypeName(name, Some(variant)))
    }

    #[inline]
    fn push_u32_length(&mut self, length: usize) -> Result<(), TraceLimitErrorKind> {
        push_u32(
            self.data,
            u32::try_from(length).map_err(|_| TraceLimitErrorKind::Values)?,
        );
        Ok(())
    }

    #[inline]
    fn push_trace(&mut self, trace: TraceNodeKind) {
        self.data.push(trace.into());
    }

    #[inline]
    fn reserve_u32(&mut self) -> Result<TraceIndex, TraceLimitErrorKind> {
        let index = TraceIndex::try_from(self.data.len())?;
        self.data.extend_from_slice(&[!0; 4]);
        Ok(index)
    }

    #[inline]
    fn push_length_bytes(&mut self, bytes: &[u8]) -> Result<(), TraceLimitErrorKind> {
        self.push_u32_length(bytes.len())?;
        self.data.extend_from_slice(bytes);
        Ok(())
    }
}

#[inline]
fn push_u32(data: &mut Vec<u8>, integer: u32) {
    data.extend_from_slice(&integer.to_le_bytes());
}

#[inline]
fn fill_reserved_bytes(data: &mut [u8], index: TraceIndex, bytes: &[u8]) {
    data[index.into()..][..bytes.len()].copy_from_slice(bytes);
}

/// Merges a node without children (a scalar, unit or `None`) into `slot`. Callers check the
/// common case, `slot` already being that node, inline.
#[inline(never)]
fn merge_leaf(slot: &mut SchemaBuilderNode, node: SchemaBuilderNode) {
    match slot {
        SchemaBuilderNode::Union(members) if !members.is_empty() => {
            if !members.contains(&node) {
                members.push(node);
            }
        }
        _ if *slot == node => {}
        _ => slot.union(node),
    }
}

/// Where a node sits within a slot: the slot itself, or a member of the union in it.
#[derive(Clone, Copy, Debug)]
enum Location {
    Itself,
    Member(usize),
}

impl Location {
    #[inline]
    fn get(self, slot: &mut SchemaBuilderNode) -> &mut SchemaBuilderNode {
        match (self, slot) {
            (Location::Member(index), SchemaBuilderNode::Union(members)) => &mut members[index],
            (_, slot) => slot,
        }
    }
}

/// Finds the member of `slot` (a non-union slot being its own only member) that `matches`
/// accepts, adding `make()` as a new member if there is none, the same way
/// [`SchemaBuilderNode::union`] would add a node that doesn't unify with any member.
#[inline]
fn locate(
    slot: &mut SchemaBuilderNode,
    matches: impl Fn(&SchemaBuilderNode) -> bool,
    make: impl FnOnce() -> SchemaBuilderNode,
) -> Location {
    match find(slot, matches) {
        Some(location) => location,
        None => insert(slot, make()),
    }
}

/// Finds the member of `slot` (a non-union slot being its own only member) that `matches`
/// accepts.
#[inline]
fn find(
    slot: &SchemaBuilderNode,
    matches: impl Fn(&SchemaBuilderNode) -> bool,
) -> Option<Location> {
    match slot {
        SchemaBuilderNode::Union(members) => members.iter().position(matches).map(Location::Member),
        _ if matches(slot) => Some(Location::Itself),
        _ => None,
    }
}

/// Adds `node` to `slot` as a new member, which none of the existing members must unify with.
#[inline]
fn insert(slot: &mut SchemaBuilderNode, node: SchemaBuilderNode) -> Location {
    match slot {
        SchemaBuilderNode::Union(members) if members.is_empty() => {
            *slot = node;
            Location::Itself
        }
        SchemaBuilderNode::Union(members) => {
            members.push(node);
            Location::Member(members.len() - 1)
        }
        _ => {
            let old = std::mem::take(slot);
            *slot = SchemaBuilderNode::Union(vec![old, node]);
            Location::Member(1)
        }
    }
}

#[inline]
fn member(
    slot: &mut SchemaBuilderNode,
    matches: impl Fn(&SchemaBuilderNode) -> bool,
    make: impl FnOnce() -> SchemaBuilderNode,
) -> &mut SchemaBuilderNode {
    locate(slot, matches, make).get(slot)
}

#[inline]
fn same_name(left: &'static str, right: &'static str) -> bool {
    std::ptr::eq(left, right) || left == right
}

/// The error of a sequence or map whose element failed to trace, when the `Serialize`
/// implementation ignored that and carried on: the failed element is already partly in the trace.
#[cold]
fn ignored_error() -> TraceError {
    TraceError::Custom("an element failed to serialize, but serialization carried on".into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SchemaBuilderNode {
    Bool,

    I8,
    I16,
    I32,
    I64,
    I128,

    U8,
    U16,
    U32,
    U64,
    U128,

    F32,
    F64,
    Char,

    String,
    Bytes,

    OptionNone,
    OptionSome(Box<SchemaBuilderNode>),

    Unit(Option<TypeName>),
    Newtype(TypeName, Box<SchemaBuilderNode>),

    Map(Box<SchemaBuilderNode>, Box<SchemaBuilderNode>),
    Sequence(Box<SchemaBuilderNode>),

    Union(Vec<SchemaBuilderNode>),

    /// Tuple, tuple struct, tuple variant, struct or struct variant.
    Record {
        name: Option<TypeName>,
        field_names: Option<FieldNameListIndex>,
        field_types: Vec<SchemaBuilderNode>,
        skippable: Vec<MemberIndex>,
        /// The names of a struct (`None` for tuples), kept alongside `name` and `field_names` so
        /// tracing can match a value to a recorded struct without interning them.
        keys: Option<StructKeys>,
    },
}

/// The `&'static str` names of a traced struct or struct variant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructKeys {
    name: &'static str,
    variant: Option<&'static str>,
    fields: Vec<&'static str>,
}

impl StructKeys {
    #[inline]
    fn is_named(&self, name: &'static str, variant: Option<&'static str>) -> bool {
        same_name(self.name, name)
            && match (self.variant, variant) {
                (None, None) => true,
                (Some(left), Some(right)) => same_name(left, right),
                _ => false,
            }
    }
}

impl SchemaBuilderNode {
    fn unify(&mut self, other: Self) -> Result<(), Self> {
        match (&mut *self, other) {
            (SchemaBuilderNode::Union(lefts), right) => {
                if lefts.is_empty() {
                    *self = right;
                } else {
                    right.add_to_nonempty_union(lefts);
                }
                Ok(())
            }
            (left, mut right @ SchemaBuilderNode::Union(_)) => {
                std::mem::swap(left, &mut right);
                left.unify(right)
            }
            (
                SchemaBuilderNode::Newtype(left_name, left_inner),
                SchemaBuilderNode::Newtype(right_name, right_inner),
            ) => {
                if *left_name == right_name {
                    left_inner.union(*right_inner);
                    Ok(())
                } else {
                    Err(SchemaBuilderNode::Newtype(right_name, right_inner))
                }
            }
            (SchemaBuilderNode::OptionSome(left), SchemaBuilderNode::OptionSome(right)) => {
                left.union(*right);
                Ok(())
            }
            (
                SchemaBuilderNode::Record {
                    name: left_name,
                    field_names: left_field_names,
                    field_types: left_field_types,
                    skippable: left_skippable,
                    ..
                },
                SchemaBuilderNode::Record {
                    name: right_name,
                    field_names: right_field_names,
                    field_types: right_field_types,
                    skippable: right_skippable,
                    keys: right_keys,
                },
            ) => {
                if (*left_name, *left_field_names, left_field_types.len())
                    == (right_name, right_field_names, right_field_types.len())
                {
                    left_field_types
                        .iter_mut()
                        .zip(right_field_types)
                        .for_each(|(left, right)| left.union(right));
                    left_skippable.extend(right_skippable);
                    left_skippable.sort_unstable();
                    left_skippable.dedup();
                    Ok(())
                } else {
                    Err(SchemaBuilderNode::Record {
                        name: right_name,
                        field_names: right_field_names,
                        field_types: right_field_types,
                        skippable: right_skippable,
                        keys: right_keys,
                    })
                }
            }
            (
                SchemaBuilderNode::Map(left_keys, left_values),
                SchemaBuilderNode::Map(right_keys, right_values),
            ) => {
                left_keys.union(*right_keys);
                left_values.union(*right_values);
                Ok(())
            }
            (SchemaBuilderNode::Sequence(left), SchemaBuilderNode::Sequence(right)) => {
                left.union(*right);
                Ok(())
            }
            (left, right) => {
                if *left == right {
                    Ok(())
                } else {
                    Err(right)
                }
            }
        }
    }

    #[inline]
    fn union(&mut self, other: Self) {
        if let Err(other) = self.unify(other) {
            let left = std::mem::take(self);
            match self {
                SchemaBuilderNode::Union(schemas) => *schemas = vec![left, other],
                _ => unreachable!(),
            }
        }
    }

    fn add_to_nonempty_union(self, lefts: &mut Vec<SchemaBuilderNode>) {
        assert!(!lefts.is_empty());
        match self {
            SchemaBuilderNode::Union(rights) => {
                rights
                    .into_iter()
                    .for_each(|right| right.add_to_nonempty_union(lefts));
            }
            right => {
                let right = lefts
                    .iter_mut()
                    .try_fold(right, |right, left| match left.unify(right) {
                        Ok(()) => Err(()),
                        Err(recovered) => Ok(recovered),
                    })
                    .ok();
                lefts.extend(right);
            }
        }
    }
}

impl Default for SchemaBuilderNode {
    #[inline]
    fn default() -> Self {
        Self::Union(Vec::new())
    }
}

impl SchemaBuilderNode {
    fn build(self, builder: &mut SchemaBuilder) -> Result<SchemaNodeIndex, TraceError> {
        let built = match self {
            SchemaBuilderNode::Bool => SchemaNode::Bool,
            SchemaBuilderNode::I8 => SchemaNode::I8,
            SchemaBuilderNode::I16 => SchemaNode::I16,
            SchemaBuilderNode::I32 => SchemaNode::I32,
            SchemaBuilderNode::I64 => SchemaNode::I64,
            SchemaBuilderNode::I128 => SchemaNode::I128,

            SchemaBuilderNode::U8 => SchemaNode::U8,
            SchemaBuilderNode::U16 => SchemaNode::U16,
            SchemaBuilderNode::U32 => SchemaNode::U32,
            SchemaBuilderNode::U64 => SchemaNode::U64,
            SchemaBuilderNode::U128 => SchemaNode::U128,

            SchemaBuilderNode::F32 => SchemaNode::F32,
            SchemaBuilderNode::F64 => SchemaNode::F64,
            SchemaBuilderNode::Char => SchemaNode::Char,

            SchemaBuilderNode::String => SchemaNode::String,
            SchemaBuilderNode::Bytes => SchemaNode::Bytes,

            SchemaBuilderNode::OptionNone => SchemaNode::OptionNone,
            SchemaBuilderNode::OptionSome(inner) => {
                let inner = inner.build(builder)?;
                SchemaNode::OptionSome(inner)
            }
            SchemaBuilderNode::Unit(None) => SchemaNode::Unit,
            SchemaBuilderNode::Unit(Some(TypeName(name, None))) => SchemaNode::UnitStruct(name),
            SchemaBuilderNode::Unit(Some(TypeName(name, Some(variant)))) => {
                SchemaNode::UnitVariant(name, variant)
            }
            SchemaBuilderNode::Newtype(type_name, inner) => {
                let inner = inner.build(builder)?;
                match type_name {
                    TypeName(name, None) => SchemaNode::NewtypeStruct(name, inner),
                    TypeName(name, Some(variant)) => {
                        SchemaNode::NewtypeVariant(name, variant, inner)
                    }
                }
            }
            SchemaBuilderNode::Map(key, value) => {
                SchemaNode::Map(key.build(builder)?, value.build(builder)?)
            }
            SchemaBuilderNode::Sequence(item) => SchemaNode::Sequence(item.build(builder)?),
            SchemaBuilderNode::Union(variants) => {
                let mut variants = variants
                    .into_iter()
                    .map(|variant| variant.build(builder))
                    .collect::<Result<Vec<_>, _>>()?;
                // Members only failed traces recorded (see the `Record` case) are bottom.
                variants.retain(|variant| !variant.is_empty());
                variants.sort_unstable();
                variants.dedup();
                if let [variant] = *variants {
                    return Ok(variant);
                }
                if variants.len()
                    > usize::try_from(u32::MAX).expect("usize must be at least 32 bits")
                {
                    return Err(TraceError::from(TraceLimitErrorKind::UnionVariants));
                }
                SchemaNode::Union(builder.node_lists.intern_from(variants)?)
            }
            SchemaBuilderNode::Record {
                keys: Some(_),
                field_names: None,
                ..
            } => {
                // A struct whose every value failed to trace: no trace refers to it.
                SchemaNode::Union(SchemaNodeListIndex::EMPTY)
            }
            SchemaBuilderNode::Record {
                name,
                field_names,
                field_types,
                mut skippable,
                ..
            } => {
                let field_types = field_types
                    .into_iter()
                    .map(|field_type| field_type.build(builder))
                    .collect::<Result<Vec<_>, _>>()?;
                // Filter out fields whose type is an empty union (bottom-typed) from the skippable
                // list. These are fields that are ALWAYS skipped, and therefore not considered
                // skippABLE.
                //
                // Struct fields are split into three categories:
                // 1. Fields that are never skipped (type != Union[], not in skip list). Do not
                //    require discriminant bits.
                // 2. Fields that are always skipped (type == Union[], not in skip list). Do not
                //    require discriminant bits.
                // 3. Fields that are sometimes skipped (type != Union[], present in skip list).
                //    Require discriminant bits.
                skippable.retain(|&index| !field_types[usize::from(index)].is_empty());
                if skippable.len() > MAX_SKIPPABLE_FIELDS {
                    return Err(TraceError::from(TraceLimitErrorKind::SkippableFields));
                }
                let field_types = builder.node_lists.intern_from(field_types)?;
                match (name, field_names) {
                    (None, None) => SchemaNode::Tuple(field_types),
                    (Some(TypeName(name, None)), None) => {
                        SchemaNode::TupleStruct(name, field_types)
                    }
                    (Some(TypeName(name, Some(variant))), None) => {
                        SchemaNode::TupleVariant(name, variant, field_types)
                    }
                    (None, Some(_field_names)) => {
                        unreachable!("anonymous structs don't exist in rust!")
                    }
                    (Some(TypeName(name, None)), Some(field_names)) => {
                        let skip_list = builder.member_lists.intern_from(skippable)?;
                        SchemaNode::Struct(name, field_names, skip_list, field_types)
                    }
                    (Some(TypeName(name, Some(variant))), Some(field_names)) => {
                        let skip_list = builder.member_lists.intern_from(skippable)?;
                        SchemaNode::StructVariant(
                            name,
                            variant,
                            field_names,
                            skip_list,
                            field_types,
                        )
                    }
                }
            }
        };
        builder.nodes.intern(built).map_err(Into::into)
    }
}

macro_rules! fn_serialize_as_u8 {
    ($(($fn_name:ident, $value_type:ty, $node:ident),)+) => {
        $(
            #[inline]
            fn $fn_name(mut self, value: $value_type) -> Result<Self::Ok, Self::Error> {
                self.push_trace(TraceNodeKind::$node);
                self.data.push(value as u8);
                if !matches!(self.slot, SchemaBuilderNode::$node) {
                    merge_leaf(self.slot, SchemaBuilderNode::$node);
                }
                Ok(())
            }
        )+
    };
}

macro_rules! fn_serialize_as_le_bytes {
    ($(($fn_name:ident, $value_type:ty, $node:ident ),)+) => {
        $(
            #[inline]
            fn $fn_name(mut self, value: $value_type) -> Result<Self::Ok, Self::Error> {
                self.push_trace(TraceNodeKind::$node);
                self.data.extend_from_slice(&value.to_le_bytes());
                if !matches!(self.slot, SchemaBuilderNode::$node) {
                    merge_leaf(self.slot, SchemaBuilderNode::$node);
                }
                Ok(())
            }
        )+
    };
}

impl<'a> Serializer for RootSerializer<'a> {
    type Ok = ();
    type Error = TraceError;

    type SerializeSeq = SequenceSchemaBuilder<'a>;
    type SerializeTuple = TupleSchemaBuilder<'a>;
    type SerializeTupleStruct = TupleSchemaBuilder<'a>;
    type SerializeTupleVariant = TupleSchemaBuilder<'a>;
    type SerializeMap = MapSchemaBuilder<'a>;
    type SerializeStruct = StructSchemaBuilder<'a>;
    type SerializeStructVariant = StructSchemaBuilder<'a>;

    fn_serialize_as_u8! {
        (serialize_bool, bool, Bool),
        (serialize_i8, i8, I8),
        (serialize_u8, u8, U8),
    }

    fn_serialize_as_le_bytes! {
        (serialize_i16, i16, I16),
        (serialize_i32, i32, I32),
        (serialize_i64, i64, I64),
        (serialize_i128, i128, I128),
        (serialize_u16, u16, U16),
        (serialize_u32, u32, U32),
        (serialize_u64, u64, U64),
        (serialize_u128, u128, U128),
        (serialize_f32, f32, F32),
        (serialize_f64, f64, F64),
    }

    #[inline]
    fn serialize_char(mut self, value: char) -> Result<Self::Ok, Self::Error> {
        self.push_trace(TraceNodeKind::Char);
        push_u32(self.data, u32::from(value));
        if !matches!(self.slot, SchemaBuilderNode::Char) {
            merge_leaf(self.slot, SchemaBuilderNode::Char);
        }
        Ok(())
    }

    #[inline]
    fn serialize_str(mut self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.push_trace(TraceNodeKind::String);
        self.push_length_bytes(value.as_bytes())?;
        if !matches!(self.slot, SchemaBuilderNode::String) {
            merge_leaf(self.slot, SchemaBuilderNode::String);
        }
        Ok(())
    }

    #[inline]
    fn serialize_bytes(mut self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.push_trace(TraceNodeKind::Bytes);
        self.push_length_bytes(value)?;
        if !matches!(self.slot, SchemaBuilderNode::Bytes) {
            merge_leaf(self.slot, SchemaBuilderNode::Bytes);
        }
        Ok(())
    }

    #[inline]
    fn serialize_none(mut self) -> Result<Self::Ok, Self::Error> {
        self.push_trace(TraceNodeKind::OptionNone);
        if !matches!(self.slot, SchemaBuilderNode::OptionNone) {
            merge_leaf(self.slot, SchemaBuilderNode::OptionNone);
        }
        Ok(())
    }

    #[inline]
    fn serialize_some<T>(mut self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.push_trace(TraceNodeKind::OptionSome);
        let SchemaBuilderNode::OptionSome(inner) = member(
            self.slot,
            |node| matches!(node, SchemaBuilderNode::OptionSome(_)),
            || SchemaBuilderNode::OptionSome(Box::default()),
        ) else {
            unreachable!("located node is not an option")
        };
        T::serialize(
            value,
            RootSerializer {
                data: self.data,
                names: self.names,
                slot: inner,
            },
        )
    }

    #[inline]
    fn serialize_unit(mut self) -> Result<Self::Ok, Self::Error> {
        self.push_trace(TraceNodeKind::Unit);
        merge_leaf(self.slot, SchemaBuilderNode::Unit(None));
        Ok(())
    }

    #[inline]
    fn serialize_unit_struct(mut self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.push_trace(TraceNodeKind::UnitStruct);
        let name = self.push_struct_name(name)?;
        merge_leaf(self.slot, SchemaBuilderNode::Unit(Some(name)));
        Ok(())
    }

    #[inline]
    fn serialize_unit_variant(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.push_trace(TraceNodeKind::UnitVariant);
        let name = self.push_variant_name(name, variant)?;
        merge_leaf(self.slot, SchemaBuilderNode::Unit(Some(name)));
        Ok(())
    }

    #[inline]
    fn serialize_newtype_struct<T>(
        mut self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.push_trace(TraceNodeKind::NewtypeStruct);
        let name = self.push_struct_name(name)?;
        self.newtype(name, value)
    }

    #[inline]
    fn serialize_newtype_variant<T>(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.push_trace(TraceNodeKind::NewtypeVariant);
        let name = self.push_variant_name(name, variant)?;
        self.newtype(name, value)
    }

    #[inline]
    fn serialize_seq(mut self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.push_trace(TraceNodeKind::Sequence);
        let reserved_length = self.reserve_u32()?;
        let SchemaBuilderNode::Sequence(item) = member(
            self.slot,
            |node| matches!(node, SchemaBuilderNode::Sequence(_)),
            || SchemaBuilderNode::Sequence(Box::default()),
        ) else {
            unreachable!("located node is not a sequence")
        };
        Ok(SequenceSchemaBuilder {
            data: self.data,
            names: self.names,
            item,
            reserved_length,
            length: 0,
            failed: false,
        })
    }

    #[inline]
    fn serialize_tuple(mut self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.push_trace(TraceNodeKind::Tuple);
        self.push_u32_length(len)?;
        Ok(self.tuple(None, len))
    }

    #[inline]
    fn serialize_tuple_struct(
        mut self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.push_trace(TraceNodeKind::TupleStruct);
        self.push_u32_length(len)?;
        let name = self.push_struct_name(name)?;
        Ok(self.tuple(Some(name), len))
    }

    #[inline]
    fn serialize_tuple_variant(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.push_trace(TraceNodeKind::TupleVariant);
        self.push_u32_length(len)?;
        let name = self.push_variant_name(name, variant)?;
        Ok(self.tuple(Some(name), len))
    }

    #[inline]
    fn serialize_map(mut self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.push_trace(TraceNodeKind::Map);
        let reserved_length = self.reserve_u32()?;
        let SchemaBuilderNode::Map(key, value) = member(
            self.slot,
            |node| matches!(node, SchemaBuilderNode::Map(_, _)),
            || SchemaBuilderNode::Map(Box::default(), Box::default()),
        ) else {
            unreachable!("located node is not a map")
        };
        Ok(MapSchemaBuilder {
            data: self.data,
            names: self.names,
            key,
            value,
            reserved_length,
            length: 0,
            failed: false,
        })
    }

    #[inline]
    fn serialize_struct(
        mut self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.push_trace(TraceNodeKind::Struct);
        StructSchemaBuilder::new(name, None, len, self)
    }

    #[inline]
    fn serialize_struct_variant(
        mut self,
        name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.push_trace(TraceNodeKind::StructVariant);
        StructSchemaBuilder::new(name, Some(variant), len, self)
    }

    #[inline]
    fn is_human_readable(&self) -> bool {
        false
    }
}

impl<'a> RootSerializer<'a> {
    #[inline]
    fn newtype<T>(self, name: TypeName, value: &T) -> Result<(), TraceError>
    where
        T: ?Sized + Serialize,
    {
        let SchemaBuilderNode::Newtype(_, inner) = member(
            self.slot,
            |node| matches!(node, SchemaBuilderNode::Newtype(other, _) if *other == name),
            || SchemaBuilderNode::Newtype(name, Box::default()),
        ) else {
            unreachable!("located node is not a newtype")
        };
        T::serialize(
            value,
            RootSerializer {
                data: self.data,
                names: self.names,
                slot: inner,
            },
        )
    }

    #[inline]
    fn tuple(self, name: Option<TypeName>, len: usize) -> TupleSchemaBuilder<'a> {
        let SchemaBuilderNode::Record { field_types, .. } = member(
            self.slot,
            |node| {
                matches!(
                    node,
                    SchemaBuilderNode::Record { name: other, keys: None, field_types, .. }
                        if *other == name && field_types.len() == len
                )
            },
            || SchemaBuilderNode::Record {
                name,
                field_names: None,
                field_types: vec![SchemaBuilderNode::default(); len],
                skippable: Vec::new(),
                keys: None,
            },
        ) else {
            unreachable!("located node is not a record")
        };
        TupleSchemaBuilder {
            data: self.data,
            names: self.names,
            fields: field_types,
            length: 0,
            failed: false,
        }
    }
}

pub(crate) struct SequenceSchemaBuilder<'a> {
    data: &'a mut Vec<u8>,
    names: &'a mut Names,
    item: &'a mut SchemaBuilderNode,
    reserved_length: TraceIndex,
    length: usize,
    /// Whether an element failed to trace; see [`ignored_error`].
    failed: bool,
}

impl SerializeSeq for SequenceSchemaBuilder<'_> {
    type Ok = ();
    type Error = TraceError;

    #[inline]
    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        self.length += 1;
        let result = T::serialize(
            value,
            RootSerializer {
                data: self.data,
                names: self.names,
                slot: self.item,
            },
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        if self.failed {
            return Err(ignored_error());
        }
        fill_reserved_bytes(
            self.data,
            self.reserved_length,
            &u32::try_from(self.length)
                .map_err(|_| TraceLimitErrorKind::Values)?
                .to_le_bytes(),
        );
        Ok(())
    }
}

pub(crate) struct MapSchemaBuilder<'a> {
    data: &'a mut Vec<u8>,
    names: &'a mut Names,
    key: &'a mut SchemaBuilderNode,
    value: &'a mut SchemaBuilderNode,
    reserved_length: TraceIndex,
    length: usize,
    /// Whether a key or value failed to trace; see [`ignored_error`].
    failed: bool,
}

impl SerializeMap for MapSchemaBuilder<'_> {
    type Ok = ();
    type Error = TraceError;

    #[inline]
    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        self.length += 1;
        let result = T::serialize(
            key,
            RootSerializer {
                data: self.data,
                names: self.names,
                slot: self.key,
            },
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    #[inline]
    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        let result = T::serialize(
            value,
            RootSerializer {
                data: self.data,
                names: self.names,
                slot: self.value,
            },
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        if self.failed {
            return Err(ignored_error());
        }
        fill_reserved_bytes(
            self.data,
            self.reserved_length,
            &u32::try_from(self.length)
                .map_err(|_| TraceLimitErrorKind::Values)?
                .to_le_bytes(),
        );
        Ok(())
    }
}

pub(crate) struct TupleSchemaBuilder<'a> {
    data: &'a mut Vec<u8>,
    names: &'a mut Names,
    /// Exactly as many as the declared length.
    fields: &'a mut [SchemaBuilderNode],
    length: usize,
    /// Whether an element failed to trace. An implementation may ignore that and carry on, even
    /// retry the element, but the failed attempt is already in the trace.
    failed: bool,
}

impl SerializeTuple for TupleSchemaBuilder<'_> {
    type Ok = ();
    type Error = TraceError;

    #[inline]
    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        let Some(slot) = self.fields.get_mut(self.length) else {
            self.failed = true;
            return Err(TraceError::LengthMismatch);
        };
        let result = T::serialize(
            value,
            RootSerializer {
                data: self.data,
                names: self.names,
                slot,
            },
        );
        if result.is_err() {
            self.failed = true;
        }
        self.length += 1;
        result
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        if self.failed || self.length != self.fields.len() {
            return Err(TraceError::LengthMismatch);
        }
        Ok(())
    }
}

impl SerializeTupleStruct for TupleSchemaBuilder<'_> {
    type Ok = ();
    type Error = TraceError;

    #[inline]
    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        <Self as SerializeTuple>::serialize_element(self, value)
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        <Self as SerializeTuple>::end(self)
    }
}

impl SerializeTupleVariant for TupleSchemaBuilder<'_> {
    type Ok = ();
    type Error = TraceError;

    #[inline]
    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        <Self as SerializeTuple>::serialize_element(self, value)
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        <Self as SerializeTuple>::end(self)
    }
}

/// Which record a [`StructSchemaBuilder`] merges fields into.
enum StructTarget {
    /// A record in the slot, recorded by earlier values (or created for this one).
    InSlot(Location),
    /// This value's fields turned out to differ from the record in the slot that has the same
    /// name, so they go into a separate record that is added to the slot at the end.
    Detached(SchemaBuilderNode),
}

struct RecordParts<'r> {
    name: &'r mut Option<TypeName>,
    field_names: &'r mut Option<FieldNameListIndex>,
    field_types: &'r mut Vec<SchemaBuilderNode>,
    skippable: &'r mut Vec<MemberIndex>,
    keys: &'r mut StructKeys,
}

impl StructTarget {
    #[inline]
    fn parts<'r>(&'r mut self, slot: &'r mut SchemaBuilderNode) -> RecordParts<'r> {
        let node = match self {
            StructTarget::InSlot(location) => location.get(slot),
            StructTarget::Detached(node) => node,
        };
        let SchemaBuilderNode::Record {
            name,
            field_names,
            field_types,
            skippable,
            keys: Some(keys),
        } = node
        else {
            unreachable!("struct target is not a struct record")
        };
        RecordParts {
            name,
            field_names,
            field_types,
            skippable,
            keys,
        }
    }

    /// Makes the target record's field `index` (the next one) `key` and returns its type,
    /// switching to a detached record if the recorded one has a different field there.
    #[inline]
    fn enter_field<'r>(
        &'r mut self,
        slot: &'r mut SchemaBuilderNode,
        growing: &mut bool,
        index: usize,
        key: &'static str,
    ) -> &'r mut SchemaBuilderNode {
        if !*growing
            && !self
                .parts(slot)
                .keys
                .fields
                .get(index)
                .is_some_and(|&known| same_name(known, key))
        {
            self.detach(slot, index);
            *growing = true;
        }
        let parts = self.parts(slot);
        if *growing {
            parts.keys.fields.push(key);
            parts.field_types.push(SchemaBuilderNode::default());
        }
        &mut parts.field_types[index]
    }

    /// Continues in a separate record seeded with the first `fields` fields of the current one.
    /// Those already include this value's types, merged in place, so the seed describes them;
    /// it may describe more (types other values had there), which is harmless.
    #[cold]
    fn detach(&mut self, slot: &mut SchemaBuilderNode, fields: usize) {
        let parts = self.parts(slot);
        let node = SchemaBuilderNode::Record {
            name: *parts.name,
            field_names: None,
            field_types: parts.field_types[..fields].to_vec(),
            skippable: parts
                .skippable
                .iter()
                .copied()
                .filter(|&field| usize::from(field) < fields)
                .collect(),
            keys: Some(StructKeys {
                name: parts.keys.name,
                variant: parts.keys.variant,
                fields: parts.keys.fields[..fields].to_vec(),
            }),
        };
        *self = StructTarget::Detached(node);
    }
}

/// Size of a struct's trace header after its names: the field name list, the number of
/// serialized fields and the presence list offset (see [`ALL_FIELDS_PRESENT`]).
const STRUCT_HEADER_SIZE: usize = 3 * std::mem::size_of::<u32>();

pub(crate) struct StructSchemaBuilder<'a> {
    data: &'a mut Vec<u8>,
    names: &'a mut Names,
    slot: &'a mut SchemaBuilderNode,
    target: StructTarget,
    /// Whether the target record is new for this value, so its fields get appended rather than
    /// matched against the recorded ones.
    growing: bool,
    /// Where the struct header starts in the trace.
    header: usize,
    /// Where this struct's skipped fields start in [`Names::skipped`].
    skipped_start: usize,
    /// Fields seen so far, serialized or skipped.
    fields: usize,
    skipped: usize,
    /// Declared number of serialized (non-skipped) fields.
    length: usize,
    /// Whether a field failed to serialize; an implementation may ignore that and carry on,
    /// which must still fail tracing.
    failed: bool,
}

impl<'a> StructSchemaBuilder<'a> {
    pub fn new(
        name: &'static str,
        variant: Option<&'static str>,
        length: usize,
        parent: RootSerializer<'a>,
    ) -> Result<Self, TraceError> {
        let RootSerializer { data, names, slot } = parent;
        // Nearly always the slot already has this struct, and with it the interned names.
        let found = find(slot, |node| {
            matches!(
                node,
                // A record without field names was only ever traced by values that failed.
                SchemaBuilderNode::Record { keys: Some(keys), field_names: Some(_), .. }
                    if keys.is_named(name, variant)
            )
        });
        let (location, type_name, growing) = match found {
            Some(location) => {
                let SchemaBuilderNode::Record {
                    name: Some(type_name),
                    ..
                } = location.get(slot)
                else {
                    unreachable!("located node is not a named record")
                };
                (location, *type_name, false)
            }
            None => {
                let type_name = TypeName(
                    names.type_names.intern(name)?,
                    variant
                        .map(|variant| names.variant_names.intern(variant))
                        .transpose()?,
                );
                let record = SchemaBuilderNode::Record {
                    name: Some(type_name),
                    field_names: None,
                    field_types: Vec::with_capacity(length),
                    skippable: Vec::new(),
                    keys: Some(StructKeys {
                        name,
                        variant,
                        fields: Vec::with_capacity(length),
                    }),
                };
                (insert(slot, record), type_name, true)
            }
        };

        push_u32(data, type_name.0.into());
        if let Some(variant) = type_name.1 {
            push_u32(data, variant.into());
        }
        let header = usize::from(TraceIndex::try_from(data.len())?);
        // Note that, maybe counter-intuitively, this `length` does NOT include skipped fields.
        // This explicitly documented by `serde`.
        let mut bytes = [!0; STRUCT_HEADER_SIZE];
        bytes[4..8].copy_from_slice(
            &u32::try_from(length)
                .map_err(|_| TraceLimitErrorKind::Values)?
                .to_le_bytes(),
        );
        data.extend_from_slice(&bytes);

        Ok(Self {
            data,
            skipped_start: names.skipped.len(),
            names,
            slot,
            target: StructTarget::InSlot(location),
            growing,
            header,
            fields: 0,
            skipped: 0,
            length,
            failed: false,
        })
    }
}

impl SerializeStruct for StructSchemaBuilder<'_> {
    type Ok = ();
    type Error = TraceError;

    #[inline]
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        if self.fields - self.skipped >= self.length {
            self.failed = true;
            return Err(TraceError::LengthMismatch);
        }
        let slot = self
            .target
            .enter_field(self.slot, &mut self.growing, self.fields, key);
        self.fields += 1;
        let result = T::serialize(
            value,
            RootSerializer {
                data: self.data,
                names: self.names,
                slot,
            },
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    #[inline]
    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        let skipped = MemberIndex::try_from(self.fields)?;
        self.target
            .enter_field(self.slot, &mut self.growing, self.fields, key);
        self.fields += 1;
        self.skipped += 1;
        self.names.skipped.push(skipped);
        let skippable = self.target.parts(self.slot).skippable;
        if let Err(position) = skippable.binary_search(&skipped) {
            skippable.insert(position, skipped);
        }
        Ok(())
    }

    #[inline]
    fn end(mut self) -> Result<Self::Ok, Self::Error> {
        if self.failed
            || self.fields - self.skipped != self.length
            || self.names.skipped.len() - self.skipped_start != self.skipped
        {
            return Err(TraceError::LengthMismatch);
        }
        if !self.growing && self.target.parts(self.slot).keys.fields.len() != self.fields {
            // The recorded struct has more fields than this value.
            self.target.detach(self.slot, self.fields);
        }

        let parts = self.target.parts(self.slot);
        let field_names = match *parts.field_names {
            Some(field_names) => field_names,
            None => {
                let names = parts
                    .keys
                    .fields
                    .iter()
                    .map(|&key| self.names.field_names.intern(key))
                    .collect::<Result<Box<[_]>, _>>()?;
                let field_names = self.names.field_name_lists.intern(names)?;
                *parts.field_names = Some(field_names);
                field_names
            }
        };
        self.data[self.header..][..4].copy_from_slice(&u32::from(field_names).to_le_bytes());

        if self.skipped > 0 {
            // The presence list: the indices of the serialized fields, after their values.
            let offset = self.data.len() - (self.header + STRUCT_HEADER_SIZE);
            let offset = u32::try_from(offset)
                .ok()
                .filter(|&offset| offset != ALL_FIELDS_PRESENT)
                .ok_or(TraceLimitErrorKind::Values)?;
            self.data[self.header + 8..][..4].copy_from_slice(&offset.to_le_bytes());
            self.data.reserve(self.length * std::mem::size_of::<u32>());
            let mut skipped = self.names.skipped[self.skipped_start..].iter().peekable();
            for field in 0..self.fields {
                if skipped
                    .next_if(|&&skipped| usize::from(skipped) == field)
                    .is_none()
                {
                    push_u32(
                        self.data,
                        u32::try_from(field).expect("field index fits u32"),
                    );
                }
            }
            self.names.skipped.truncate(self.skipped_start);
        }

        if let StructTarget::Detached(node) = self.target {
            self.slot.union(node);
        }
        Ok(())
    }
}

impl SerializeStructVariant for StructSchemaBuilder<'_> {
    type Ok = ();
    type Error = TraceError;

    #[inline]
    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + serde::Serialize,
    {
        <Self as SerializeStruct>::serialize_field(self, key, value)
    }

    #[inline]
    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        <Self as SerializeStruct>::skip_field(self, key)
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        <Self as SerializeStruct>::end(self)
    }
}
