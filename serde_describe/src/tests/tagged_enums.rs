use crate::SelfDescribed;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum Shape {
    Circle { radius: f32 },
    Rect { w: f32, h: f32 },
    Empty,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
enum Adjacent {
    Id(u32),
    Name(String),
}

#[test]
fn internally_tagged_enum_round_trips() {
    let value = vec![
        Shape::Circle { radius: 1.0 },
        Shape::Rect { w: 2.0, h: 3.0 },
        Shape::Empty,
    ];
    let bytes = bitcode::serialize(&SelfDescribed(&value)).unwrap();
    let decoded = bitcode::deserialize::<SelfDescribed<Vec<Shape>>>(&bytes)
        .unwrap()
        .0;
    assert_eq!(decoded, value);
}

#[test]
fn adjacently_tagged_enum_round_trips() {
    let value = vec![Adjacent::Id(7), Adjacent::Name("x".into())];
    let bytes = bitcode::serialize(&SelfDescribed(&value)).unwrap();
    let decoded = bitcode::deserialize::<SelfDescribed<Vec<Adjacent>>>(&bytes)
        .unwrap()
        .0;
    assert_eq!(decoded, value);
}
