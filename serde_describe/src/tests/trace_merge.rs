//! Tracing merges each value's type into the schema in place; these cover the cases where a
//! value doesn't match what was recorded before it.

use super::helpers::{check_roundtrip, from_self_described_bitcode, to_self_described_bitcode};
use crate::{DescribedBy, SchemaBuilder, TraceError};
use serde::{
    Deserialize, Serialize, Serializer,
    ser::{Error as _, SerializeSeq, SerializeStruct},
};
use std::collections::BTreeMap;

/// Serializes as a struct named `Variable` with the given fields, in order.
struct Variable(Vec<(&'static str, u32)>);

impl Serialize for Variable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Variable", self.0.len())?;
        for &(key, value) in &self.0 {
            state.serialize_field(key, &value)?;
        }
        state.end()
    }
}

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Decoded {
    a: u32,
    b: u32,
    c: u32,
}

#[test]
fn same_struct_name_with_different_fields() {
    let values = vec![
        Variable(vec![("a", 1), ("b", 2)]),
        // Fewer fields than recorded.
        Variable(vec![("a", 3)]),
        // Same fields, different order.
        Variable(vec![("b", 4), ("a", 5)]),
        // More fields than recorded.
        Variable(vec![("a", 6), ("b", 7), ("c", 8)]),
        Variable(vec![]),
        Variable(vec![("a", 9), ("b", 10)]),
        Variable(vec![("b", 11), ("a", 12)]),
    ];
    let bytes = to_self_described_bitcode(&values);
    let decoded = from_self_described_bitcode::<Vec<Decoded>>(&bytes).unwrap();
    let expected = [
        (1, 2, 0),
        (3, 0, 0),
        (5, 4, 0),
        (6, 7, 8),
        (0, 0, 0),
        (9, 10, 0),
        (12, 11, 0),
    ]
    .map(|(a, b, c)| Decoded { a, b, c });
    assert_eq!(decoded, expected);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Inner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<u8>,
    y: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    z: Vec<Inner>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Outer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    a: Option<u8>,
    inner: Inner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    b: Option<String>,
}

fn inner(x: Option<u8>, y: u8, z: Vec<Inner>) -> Inner {
    Inner { x, y, z }
}

#[test]
fn skipped_fields_vary_across_values() {
    let mut values = Vec::new();
    for a in [None, Some(1)] {
        for b in [None, Some("b".to_string())] {
            for x in [None, Some(2)] {
                values.push(Outer {
                    a,
                    inner: inner(
                        x,
                        3,
                        vec![inner(None, 4, vec![]), inner(Some(5), 6, vec![])],
                    ),
                    b: b.clone(),
                });
                values.push(Outer {
                    a,
                    inner: inner(x, 7, vec![]),
                    b: b.clone(),
                });
            }
        }
    }
    check_roundtrip(&values);
    // Every value alone: all fields present, or skipped ones only.
    for value in &values {
        check_roundtrip(value);
    }
}

/// A field that is always skipped has no type, so the struct has no skippable fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct AlwaysSkipped {
    a: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    never: Option<u8>,
    b: f32,
}

#[test]
fn always_skipped_field() {
    let value = AlwaysSkipped {
        a: 1,
        never: None,
        b: 2.5,
    };
    check_roundtrip(&value);
    check_roundtrip(&vec![value.clone(), value.clone()]);
    check_roundtrip(&vec![(value.clone(), 3u8), (value, 4u8)]);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Newtype(u32);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Shape {
    Unit,
    Newtype(Newtype),
    Tuple(u8, u16),
    Struct {
        x: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f32>,
    },
    Nested(Box<Shape>),
}

#[test]
fn unions_of_records_and_enums() {
    let shapes = vec![
        Shape::Struct { x: 1.0, y: None },
        Shape::Unit,
        Shape::Tuple(1, 2),
        Shape::Newtype(Newtype(3)),
        Shape::Struct {
            x: 2.0,
            y: Some(3.0),
        },
        Shape::Nested(Box::new(Shape::Struct { x: 4.0, y: None })),
        Shape::Nested(Box::new(Shape::Nested(Box::new(Shape::Unit)))),
    ];
    check_roundtrip(&shapes);
    let maps: Vec<BTreeMap<String, Option<Shape>>> = vec![
        BTreeMap::new(),
        BTreeMap::from([("a".into(), None), ("b".into(), Some(Shape::Unit))]),
        BTreeMap::from([("c".into(), Some(Shape::Tuple(3, 4)))]),
    ];
    check_roundtrip(&maps);
}

/// Serializes as a `Thing` whose field `b` fails half way, after an element was traced.
struct Failing;

impl Serialize for Failing {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element(&1u64)?;
        Err(S::Error::custom("failing"))
    }
}

struct BadThing;

impl Serialize for BadThing {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Thing", 2)?;
        state.skip_field("skipped")?;
        state.serialize_field("a", &1u32)?;
        state.serialize_field("b", &Failing)?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Thing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skipped: Option<u8>,
    a: u32,
    b: Vec<String>,
}

#[test]
fn failed_trace_leaves_a_usable_schema() {
    let good = [
        Thing {
            skipped: None,
            a: 1,
            b: vec!["x".into()],
        },
        Thing {
            skipped: Some(2),
            a: 3,
            b: vec![],
        },
    ];
    let mut builder = SchemaBuilder::new();
    assert!(matches!(
        builder.trace(&BadThing),
        Err(TraceError::Custom(_))
    ));
    let first = builder.trace(&good[0]).unwrap();
    assert!(builder.trace(&BadThing).is_err());
    let second = builder.trace(&good[1]).unwrap();
    let schema = builder.build().unwrap();
    for (trace, value) in [(&first, &good[0]), (&second, &good[1])] {
        let bytes = bitcode::serialize(&(&schema, DescribedBy(trace, &schema))).unwrap();
        assert_eq!(
            &from_self_described_bitcode::<Thing>(&bytes).unwrap(),
            value
        );
    }
}

/// A tuple struct with the same name as [`Thing`] and as many fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Thing")]
struct ThingTuple(u8, u32, Vec<String>);

#[test]
fn failed_struct_does_not_collide_with_tuple_struct() {
    let mut builder = SchemaBuilder::new();
    assert!(builder.trace(&[BadThing]).is_err());
    let value = [ThingTuple(1, 2, vec!["x".into()])];
    let trace = builder.trace(&value).unwrap();
    let schema = builder.build().unwrap();
    let bytes = bitcode::serialize(&(&schema, DescribedBy(&trace, &schema))).unwrap();
    assert_eq!(
        from_self_described_bitcode::<[ThingTuple; 1]>(&bytes).unwrap(),
        value
    );
}

/// Ignores the error of every failing element and carries on.
struct Swallowing(Collection);

enum Collection {
    Seq,
    Map,
    Tuple,
}

impl Serialize for Swallowing {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeTuple};
        match self.0 {
            Collection::Seq => {
                let mut seq = serializer.serialize_seq(None)?;
                let _ = seq.serialize_element(&Failing);
                seq.serialize_element(&2u8)?;
                seq.end()
            }
            Collection::Map => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_key(&1u8)?;
                let _ = map.serialize_value(&Failing);
                map.end()
            }
            Collection::Tuple => {
                // Retries the failed element.
                let mut tuple = serializer.serialize_tuple(1)?;
                let _ = tuple.serialize_element(&Failing);
                tuple.serialize_element(&2u8)?;
                tuple.end()
            }
        }
    }
}

#[test]
fn ignored_element_errors_fail_the_trace() {
    for collection in [Collection::Seq, Collection::Map, Collection::Tuple] {
        assert!(SchemaBuilder::new().trace(&Swallowing(collection)).is_err());
    }
}
