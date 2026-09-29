//! Generated API and protobuf round-trip tests for `common.device.Value`.

use prost::Message;

use crate::common::device::{Value, value};

fn round_trip(value: Value) -> Value {
    let encoded = value.encode_to_vec();
    Value::decode(encoded.as_slice()).expect("Value protobuf must decode")
}

#[test]
fn generated_api_exposes_64_bit_value_variants() {
    let _ = value::Value::Int(i64::MIN);
    let _ = value::Value::Uint(u64::MAX);
    let _ = value::Value::IntArr(value::Int64Array { value: vec![] });
    let _ = value::Value::UintArr(value::Uint64Array { value: vec![] });
}

#[test]
fn int_round_trips_i64_boundaries() {
    for expected in [i64::MIN, i64::MAX] {
        let decoded = round_trip(Value {
            value: Some(value::Value::Int(expected)),
        });

        assert_eq!(decoded.value, Some(value::Value::Int(expected)));
    }
}

#[test]
fn uint_round_trips_u64_max() {
    let decoded = round_trip(Value {
        value: Some(value::Value::Uint(u64::MAX)),
    });

    assert_eq!(decoded.value, Some(value::Value::Uint(u64::MAX)));
}

#[test]
fn int_array_round_trips_i64_boundaries() {
    let expected = vec![i64::MIN, 0, i64::MAX];
    let decoded = round_trip(Value {
        value: Some(value::Value::IntArr(value::Int64Array {
            value: expected.clone(),
        })),
    });

    assert_eq!(
        decoded.value,
        Some(value::Value::IntArr(value::Int64Array { value: expected }))
    );
}

#[test]
fn uint_array_round_trips_u64_max() {
    let expected = vec![0, u64::MAX];
    let decoded = round_trip(Value {
        value: Some(value::Value::UintArr(value::Uint64Array {
            value: expected.clone(),
        })),
    });

    assert_eq!(
        decoded.value,
        Some(value::Value::UintArr(value::Uint64Array {
            value: expected
        }))
    );
}

#[test]
fn existing_value_variants_still_round_trip() {
    let cases = [
        Value {
            value: Some(value::Value::Scalar(42.5)),
        },
        Value {
            value: Some(value::Value::ScalarArr(value::ScalarArray {
                value: vec![1.5, 2.5],
            })),
        },
        Value {
            value: Some(value::Value::Raw(vec![1, 2, 3])),
        },
        Value {
            value: Some(value::Value::Text("unchanged".to_owned())),
        },
        Value {
            value: Some(value::Value::TextArr(value::TextArray {
                value: vec!["first".to_owned(), "second".to_owned()],
            })),
        },
        Value {
            value: Some(value::Value::AnaAlarm(value::AnalogAlarm {
                minimum: -1.0,
                maximum: 1.0,
                alarm_enable: true,
                alarm_status: false,
                abort: false,
                abort_inhibit: true,
                tries_needed: 2,
                tries_now: 1,
            })),
        },
        Value {
            value: Some(value::Value::DigAlarm(value::DigitalAlarm {
                nominal: -1,
                mask: 0xff,
                alarm_enable: true,
                alarm_status: true,
                abort: false,
                abort_inhibit: false,
                tries_needed: 3,
                tries_now: 2,
            })),
        },
        Value {
            value: Some(value::Value::BasicStatus(value::BasicStatus {
                value: [("status".to_owned(), "ok".to_owned())]
                    .into_iter()
                    .collect(),
            })),
        },
    ];

    for expected in cases {
        assert_eq!(round_trip(expected.clone()), expected);
    }
}
