use notes_core::{Hlc, Op, OpKind, PageDelete};

fn operation() -> Op {
    Op {
        op_id: uuid::Uuid::from_u128(2),
        workspace_uuid: uuid::Uuid::from_u128(1),
        device_id: uuid::Uuid::from_u128(3),
        hlc: Hlc::new(1, 0, uuid::Uuid::from_u128(3)),
        format_version: notes_core::operation::FORMAT_VERSION,
        kind: OpKind::PageDelete(PageDelete {
            uuid: uuid::Uuid::from_u128(1),
        }),
    }
}

fn fixture(name: &str) -> &'static str {
    match name {
        "op_v1" => include_str!("fixtures/persisted_op_v1.json"),
        "op_v2" => include_str!("fixtures/persisted_op_v2.json"),
        "history_v1" => include_str!("fixtures/persisted_history_v1.json"),
        "history_v2" => include_str!("fixtures/persisted_history_v2.json"),
        _ => unreachable!("unknown fixture"),
    }
}

#[test]
fn persisted_operation_envelope_reads_v1_and_writes_v2_golden_shape() {
    let expected = operation();
    assert_eq!(
        notes_core::decode_persisted_envelope::<Op>(fixture("op_v1")).unwrap(),
        expected
    );
    assert_eq!(
        notes_core::decode_persisted_envelope::<Op>(fixture("op_v2")).unwrap(),
        expected
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &notes_core::encode_persisted_envelope(&expected).unwrap()
        )
        .unwrap(),
        serde_json::from_str::<serde_json::Value>(fixture("op_v2")).unwrap()
    );
}

#[test]
fn persisted_history_envelope_reads_v1_and_writes_v2_golden_shape() {
    let expected = vec![operation().kind];
    assert_eq!(
        notes_core::decode_persisted_envelope::<Vec<OpKind>>(fixture("history_v1")).unwrap(),
        expected
    );
    assert_eq!(
        notes_core::decode_persisted_envelope::<Vec<OpKind>>(fixture("history_v2")).unwrap(),
        expected
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &notes_core::encode_persisted_envelope(&expected).unwrap()
        )
        .unwrap(),
        serde_json::from_str::<serde_json::Value>(fixture("history_v2")).unwrap()
    );
}

#[test]
fn persisted_envelope_rejects_unknown_tagged_versions() {
    let unknown = r#"{"format_version":999,"payload":[]}"#;
    let error = notes_core::decode_persisted_envelope::<Vec<OpKind>>(unknown).unwrap_err();
    assert!(error.to_string().contains("unsupported persisted envelope"));
}
