//! Sequences whose elements may be decoded straight through the inner deserializer
//! (`raw-seq-elements`) must decode the same values as the schema layer would, including when
//! the target type doesn't match the schema exactly.
use crate::SelfDescribed;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fmt::Debug;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Vertex {
    pos: [f32; 3],
    uv: (f32, f32),
    id: Id,
    name: String,
    unit: (),
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Id(u32);

fn roundtrip_all<T>(value: &T)
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

    let text = ron::to_string(&SelfDescribed(value)).unwrap();
    assert_eq!(&ron::from_str::<SelfDescribed<T>>(&text).unwrap().0, value);
}

fn convert<FromT, IntoT>(value: &FromT) -> IntoT
where
    FromT: Serialize,
    IntoT: DeserializeOwned + PartialEq + Debug,
{
    let bytes = bitcode::serialize(&SelfDescribed(value)).unwrap();
    let from_bitcode = bitcode::deserialize::<SelfDescribed<IntoT>>(&bytes)
        .unwrap()
        .0;
    let bytes = postcard::to_stdvec(&SelfDescribed(value)).unwrap();
    let from_postcard = postcard::from_bytes::<SelfDescribed<IntoT>>(&bytes)
        .unwrap()
        .0;
    assert_eq!(from_bitcode, from_postcard);
    from_bitcode
}

#[test]
fn fixed_shape_elements_roundtrip() {
    roundtrip_all(&(0..100u8).collect::<Vec<u8>>());
    roundtrip_all(&(0..100).map(|i| i as f32 * 0.5).collect::<Vec<f32>>());
    roundtrip_all(&(0..100).map(|i| [i as f32, 1.0, -1.0]).collect::<Vec<_>>());
    roundtrip_all(
        &(0..100)
            .map(|i| V3 {
                x: i as f32,
                y: 0.5,
                z: -1.0,
            })
            .collect::<Vec<_>>(),
    );
    roundtrip_all(
        &(0..100)
            .map(|i| Vertex {
                pos: [i as f32, 2.0, 3.0],
                uv: (0.25, i as f32),
                id: Id(i),
                name: format!("v{i}"),
                unit: (),
            })
            .collect::<Vec<_>>(),
    );
    roundtrip_all(&vec![vec![1u32, 2], vec![], vec![3, 4, 5]]);
    roundtrip_all(&vec![Vec::<u8>::new(), vec![1, 2, 3]]);
}

#[test]
fn widened_numbers_decode_through_the_schema_layer() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Wide {
        x: f64,
        y: f64,
        z: f64,
    }
    let value: Vec<V3> = (0..10)
        .map(|i| V3 {
            x: i as f32,
            y: 0.5,
            z: -1.0,
        })
        .collect();
    let wide: Vec<Wide> = convert(&value);
    assert_eq!(wide.len(), 10);
    assert!(
        wide.iter()
            .enumerate()
            .all(|(i, v)| v.x == i as f64 && v.y == 0.5 && v.z == -1.0)
    );

    let bytes: Vec<u8> = (0..=255).collect();
    let words: Vec<u16> = convert(&bytes);
    assert_eq!(words, (0..=255).collect::<Vec<u16>>());
}

#[test]
fn reordered_fields_decode_through_the_schema_layer() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Reordered {
        z: f32,
        x: f32,
        y: f32,
    }
    let value: Vec<V3> = (0..10)
        .map(|i| V3 {
            x: i as f32,
            y: 0.5,
            z: -1.0,
        })
        .collect();
    let reordered: Vec<Reordered> = convert(&value);
    assert!(
        reordered
            .iter()
            .enumerate()
            .all(|(i, v)| v.x == i as f32 && v.y == 0.5 && v.z == -1.0)
    );
}

#[test]
fn optional_targets_decode_through_the_schema_layer() {
    let value: Vec<u32> = (0..10).collect();
    let optional: Vec<Option<u32>> = convert(&value);
    assert_eq!(optional, (0..10).map(Some).collect::<Vec<_>>());

    let tuples: Vec<(u8, u8)> = (0..10).map(|i| (i, i + 1)).collect();
    let arrays: Vec<[u16; 2]> = convert(&tuples);
    assert_eq!(arrays, (0..10).map(|i| [i, i + 1]).collect::<Vec<_>>());
}

std::thread_local! {
    // Already `const`: clippy 1.98 flags it regardless.
    #[allow(clippy::missing_const_for_thread_local)]
    static SEEN: std::cell::RefCell<Vec<bool>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Records, per decoded value, whether it was handed the schema layer's deserializer.
#[derive(Debug, PartialEq, Serialize)]
struct Spy(u32);

impl<'de> Deserialize<'de> for Spy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let schema_layer = std::any::type_name::<D>().contains("SchemaDeserializer");
        SEEN.with(|seen| seen.borrow_mut().push(schema_layer));
        #[derive(Deserialize)]
        struct Newtype(u32);
        Newtype::deserialize(deserializer).map(|Newtype(value)| Spy(value))
    }
}

#[test]
fn raw_path_is_taken_only_after_an_exact_first_element() {
    let value: Vec<Spy> = (0..20).map(Spy).collect();
    let bytes = bitcode::serialize(&SelfDescribed(&value)).unwrap();

    SEEN.with(|seen| seen.borrow_mut().clear());
    let decoded = bitcode::deserialize::<SelfDescribed<Vec<Spy>>>(&bytes)
        .unwrap()
        .0;
    assert_eq!(decoded, value);
    let expected = if cfg!(feature = "raw-seq-elements") {
        [true].into_iter().chain([false; 19]).collect()
    } else {
        vec![true; 20]
    };
    assert_eq!(SEEN.with(|seen| seen.take()), expected);

    // Human-readable formats always go through the schema layer.
    let text = ron::to_string(&SelfDescribed(&value)).unwrap();
    SEEN.with(|seen| seen.borrow_mut().clear());
    ron::from_str::<SelfDescribed<Vec<Spy>>>(&text).unwrap();
    assert_eq!(SEEN.with(|seen| seen.take()), vec![true; 20]);

    // Short sequences always go through the schema layer.
    let short: Vec<Spy> = (0..5).map(Spy).collect();
    let bytes = bitcode::serialize(&SelfDescribed(&short)).unwrap();
    SEEN.with(|seen| seen.borrow_mut().clear());
    bitcode::deserialize::<SelfDescribed<Vec<Spy>>>(&bytes).unwrap();
    assert_eq!(SEEN.with(|seen| seen.take()), vec![true; 5]);
}
