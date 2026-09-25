use crate::SelfDescribed;
use serde::{Deserialize, Deserializer, Serialize, de};

/// A hand-written struct visitor that only understands maps, reached via `deserialize_struct`
/// with written fields in exactly the declared order. Must decode unless `positional-structs`
/// is enabled (that feature assumes every such visitor also implements `visit_seq`).
#[derive(Debug, PartialEq, Serialize)]
struct MapOnly {
    id: u32,
    generation: i32,
}

impl<'de> Deserialize<'de> for MapOnly {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = MapOnly;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("struct MapOnly")
            }
            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<MapOnly, A::Error> {
                let (mut id, mut generation) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => id = Some(map.next_value()?),
                        "generation" => generation = Some(map.next_value()?),
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(MapOnly {
                    id: id.ok_or_else(|| de::Error::missing_field("id"))?,
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                })
            }
        }
        deserializer.deserialize_struct("MapOnly", &["id", "generation"], V)
    }
}

#[test]
fn map_only_visitor_in_declared_order() {
    let value = vec![
        MapOnly {
            id: 1,
            generation: 2,
        },
        MapOnly {
            id: 3,
            generation: 4,
        },
    ];
    let bytes = bitcode::serialize(&SelfDescribed(&value)).unwrap();
    let decoded = bitcode::deserialize::<SelfDescribed<Vec<MapOnly>>>(&bytes);
    if cfg!(feature = "positional-structs") {
        assert!(
            decoded.is_err(),
            "positional-structs is documented to reject map-only visitors"
        );
    } else {
        assert_eq!(decoded.unwrap().0, value);
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Inner {
    a: f32,
    b: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Written {
    x: u8,
    inner: Vec<Inner>,
    y: f64,
}

/// Same shape as `Written` but a different field order: must go through the by-name path.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Reordered {
    y: f64,
    inner: Vec<Inner>,
    x: u8,
}

/// Numbers widen through the non-exact (slow) number path.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Widened {
    x: u64,
    inner: Vec<Inner>,
    y: f64,
}

#[test]
fn exact_reordered_and_widened_targets_share_one_schema() {
    let value = Written {
        x: 7,
        inner: vec![
            Inner {
                a: 1.5,
                b: "p".into(),
            },
            Inner {
                a: -2.0,
                b: "q".into(),
            },
        ],
        y: 0.25,
    };
    let bytes = bitcode::serialize(&SelfDescribed(&value)).unwrap();

    let exact = bitcode::deserialize::<SelfDescribed<Written>>(&bytes)
        .unwrap()
        .0;
    assert_eq!(exact, value);

    let reordered = bitcode::deserialize::<SelfDescribed<Reordered>>(&bytes)
        .unwrap()
        .0;
    assert_eq!((reordered.x, reordered.y), (7, 0.25));
    assert_eq!(reordered.inner, value.inner);

    let widened = bitcode::deserialize::<SelfDescribed<Widened>>(&bytes)
        .unwrap()
        .0;
    assert_eq!((widened.x, widened.y), (7, 0.25));
}
