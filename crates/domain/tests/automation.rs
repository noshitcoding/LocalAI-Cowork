use cowork_contracts::{EventPredicate, SubmitAutomationEvent};
use cowork_domain::automation::{matches_event, validate_event, validate_predicates};
use serde_json::json;

#[test]
fn predicates_are_typed_and_missing_fields_never_compare_equal_to_null() {
    let data =
        json!({"invoice":{"amount":150,"sender":"Acme GmbH","approved":false},"a/b":{"~key":null}});
    assert!(
        matches_event(
            &[
                EventPredicate::GreaterThan {
                    pointer: "/invoice/amount".into(),
                    value: 100.0
                },
                EventPredicate::LessThan {
                    pointer: "/invoice/amount".into(),
                    value: 200.0
                },
                EventPredicate::Contains {
                    pointer: "/invoice/sender".into(),
                    text: "GmbH".into()
                },
                EventPredicate::Equals {
                    pointer: "/invoice/approved".into(),
                    value: json!(false)
                },
                EventPredicate::Exists {
                    pointer: "/a~1b/~0key".into()
                },
            ],
            &data
        )
        .unwrap()
    );
    assert!(
        !matches_event(
            &[EventPredicate::Equals {
                pointer: "/missing".into(),
                value: json!(null)
            }],
            &data
        )
        .unwrap()
    );
    assert!(
        !matches_event(
            &[EventPredicate::GreaterThan {
                pointer: "/invoice/sender".into(),
                value: 0.0
            }],
            &data
        )
        .unwrap()
    );
}

#[test]
fn malformed_and_unbounded_filters_are_rejected() {
    for pointer in ["invoice", "/bad~", "/bad~2", "/bad\n"] {
        assert!(
            validate_predicates(&[EventPredicate::Exists {
                pointer: pointer.into()
            }])
            .is_err()
        );
    }
    assert!(
        validate_predicates(&[EventPredicate::Equals {
            pointer: "/a".into(),
            value: json!({"nested":true})
        }])
        .is_err()
    );
    assert!(
        validate_predicates(&[EventPredicate::GreaterThan {
            pointer: "/a".into(),
            value: f64::NAN
        }])
        .is_err()
    );
    assert!(
        validate_predicates(&vec![
            EventPredicate::Exists {
                pointer: "/a".into()
            };
            33
        ])
        .is_err()
    );
}

#[test]
fn event_limits_include_depth_nodes_and_encoded_size() {
    let mut event = SubmitAutomationEvent {
        event_id: "sender-event-1".into(),
        data: json!({"amount":42}),
    };
    assert!(validate_event(&event).is_ok());
    event.data = json!({"text":"x".repeat(32_768)});
    assert!(validate_event(&event).is_err());
    event.data = json!({"items":vec![0;4096]});
    assert!(validate_event(&event).is_err());
    let mut deep = json!(true);
    for _ in 0..34 {
        deep = json!({"child":deep});
    }
    event.data = deep;
    assert!(validate_event(&event).is_err());
    event.data = json!([]);
    assert!(validate_event(&event).is_err());
}
