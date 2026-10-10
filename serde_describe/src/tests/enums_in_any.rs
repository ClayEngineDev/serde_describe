//! Enums reached through `deserialize_any`. Internally tagged enums, `untagged` and `flatten`
//! buffer their input into serde's `Content`, which decodes an enum only from a variant name
//! (unit variants) or a single-key map (all others); `serde_json::Value` expects the same shapes.
use crate::SelfDescribed;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fmt::Debug;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Hold,
    Linear,
    Cubic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Shape {
    Empty,
    Radius(u32),
    Size(u32, u32),
    Named { name: String },
    Rect { w: u32, h: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Key {
    time: u32,
    mode: Mode,
    shape: Shape,
    hint: Option<Mode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Keys {
    keys: Vec<Key>,
}

/// Same shape as a timeline `Track`: internally tagged, newtype variants around structs that
/// hold enums.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "value_type", rename_all = "snake_case")]
enum Track {
    A(Keys),
    B(Keys),
    Flag { mode: Mode, shape: Shape },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Loose {
    Number(u32),
    Keyed(Key),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Settings {
    mode: Mode,
    shape: Shape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Flattened {
    id: u32,
    #[serde(flatten)]
    settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Wrapper(Mode);

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

fn shapes() -> [Shape; 5] {
    [
        Shape::Empty,
        Shape::Radius(3),
        Shape::Size(4, 5),
        Shape::Named { name: "n".into() },
        Shape::Rect { w: 6, h: 7 },
    ]
}

fn keys(seed: u32) -> Keys {
    let modes = [Mode::Hold, Mode::Linear, Mode::Cubic];
    Keys {
        keys: (0..10)
            .map(|i| Key {
                time: seed + i,
                mode: modes[i as usize % 3],
                shape: shapes()[(seed + i) as usize % 5].clone(),
                hint: (i % 2 == 0).then_some(modes[(i as usize + 1) % 3]),
            })
            .collect(),
    }
}

#[test]
fn internally_tagged_holding_enums() {
    roundtrip(&vec![
        Track::A(keys(0)),
        Track::B(keys(3)),
        Track::Flag {
            mode: Mode::Linear,
            shape: Shape::Named { name: "x".into() },
        },
    ]);
    // A single variant everywhere, so no union in the element schema.
    roundtrip(&vec![Track::A(keys(1)), Track::A(keys(2))]);
}

#[test]
fn every_variant_kind_in_a_single_field_buffer() {
    // Struct variants with one field are the case a fields-only map would misread as a variant.
    for shape in shapes() {
        roundtrip(&Track::Flag {
            mode: Mode::Cubic,
            shape,
        });
    }
}

#[test]
fn untagged_holding_enums() {
    roundtrip(
        &keys(0)
            .keys
            .into_iter()
            .enumerate()
            .map(|(i, key)| {
                if i % 3 == 0 {
                    Loose::Number(i as u32)
                } else {
                    Loose::Keyed(key)
                }
            })
            .collect::<Vec<_>>(),
    );
}

#[test]
fn flattened_enums() {
    roundtrip(
        &shapes()
            .into_iter()
            .enumerate()
            .map(|(id, shape)| Flattened {
                id: id as u32,
                settings: Settings {
                    mode: Mode::Hold,
                    shape,
                },
            })
            .collect::<Vec<_>>(),
    );
}

#[test]
fn skipped_fields_holding_enums() {
    #[derive(Serialize)]
    struct Written {
        a: u32,
        extra: Vec<Shape>,
        track: Track,
        b: u32,
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct Read {
        a: u32,
        b: u32,
    }

    let written = Written {
        a: 1,
        extra: shapes().to_vec(),
        track: Track::A(keys(0)),
        b: 2,
    };
    let bytes = bitcode::serialize(&SelfDescribed(&written)).unwrap();
    assert_eq!(
        bitcode::deserialize::<SelfDescribed<Read>>(&bytes)
            .unwrap()
            .0,
        Read { a: 1, b: 2 }
    );
}

#[test]
fn json_value_matches_json_encoding() {
    fn check<T: Serialize>(value: &T) {
        let bytes = bitcode::serialize(&SelfDescribed(value)).unwrap();
        assert_eq!(
            bitcode::deserialize::<SelfDescribed<serde_json::Value>>(&bytes)
                .unwrap()
                .0,
            serde_json::to_value(value).unwrap()
        );
    }

    check(&shapes());
    check(&keys(0));
    check(&Track::A(keys(1)));
    check(&Wrapper(Mode::Linear));
    check(&vec![Some(Mode::Hold), None]);
}
