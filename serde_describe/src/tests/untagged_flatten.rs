//! `#[serde(untagged)]` and `#[serde(flatten)]` buffer their input through `deserialize_any`
//! and maps, so they must keep working with the positional and raw-element fast paths, also
//! when the schema alone looks like it would allow them.
use crate::SelfDescribed;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, fmt::Debug};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Value {
    Number(u32),
    Text(String),
    Pair { a: u32, b: f32 },
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Inner {
    x: f32,
    y: f32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Flattened {
    id: u32,
    #[serde(flatten)]
    inner: Inner,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct WithExtra {
    id: u32,
    #[serde(flatten)]
    extra: BTreeMap<String, u32>,
}

/// Every field has a fixed-shape schema, but `value` decodes through `deserialize_any`.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Holder {
    id: u32,
    value: Value,
}

fn roundtrip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let bytes = bitcode::serialize(&SelfDescribed(value)).unwrap();
    assert_eq!(
        &bitcode::deserialize::<SelfDescribed<T>>(&bytes).unwrap().0,
        value
    );

    let bytes = postcard::to_stdvec(&SelfDescribed(value)).unwrap();
    assert_eq!(
        &postcard::from_bytes::<SelfDescribed<T>>(&bytes).unwrap().0,
        value
    );
}

#[test]
fn untagged_mixed_variants() {
    roundtrip(
        &(0..20)
            .map(|i| match i % 3 {
                0 => Value::Number(i),
                1 => Value::Text(i.to_string()),
                _ => Value::Pair {
                    a: i,
                    b: i as f32 * 0.5,
                },
            })
            .collect::<Vec<_>>(),
    );
}

#[test]
fn untagged_single_variant_sequences() {
    // All elements serialize with the same variant, so the element schema is a plain number or
    // struct rather than a union.
    roundtrip(&(0..20).map(Value::Number).collect::<Vec<_>>());
    roundtrip(
        &(0..20)
            .map(|a| Value::Pair { a, b: 1.5 })
            .collect::<Vec<_>>(),
    );
}

#[test]
fn untagged_field_in_fixed_shape_struct() {
    roundtrip(
        &(0..20)
            .map(|id| Holder {
                id,
                value: Value::Number(id * 2),
            })
            .collect::<Vec<_>>(),
    );
}

#[test]
fn flattened_structs() {
    roundtrip(
        &(0..20)
            .map(|id| Flattened {
                id,
                inner: Inner {
                    x: id as f32,
                    y: -(id as f32),
                },
            })
            .collect::<Vec<_>>(),
    );
}

#[test]
fn flattened_maps() {
    roundtrip(
        &(0..20)
            .map(|id| WithExtra {
                id,
                extra: (0..id % 4).map(|k| (format!("k{k}"), k)).collect(),
            })
            .collect::<Vec<_>>(),
    );
}
