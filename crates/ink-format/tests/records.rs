use ink_format::{
    cbor::{self, Value},
    record::{ObjectRef, Record, kind},
};
#[test]
fn deterministic_cbor_preserves_float_bits_and_unknown_extensions() {
    let values = [0.0, -0.0, 1.25, f64::from_bits(1), f64::MAX];
    let mut record = Record::new(
        kind::PAGE,
        [1; 16],
        cbor::map([(0, cbor::array(values.into_iter().map(cbor::float)))]),
    );
    record.extensions = cbor::map([(0x8000, cbor::map([(0, cbor::bytes([1, 2, 3]))]))]);
    record.required_features = vec![1, 99];
    record.dependencies = vec![[2; 16]];
    record.resources = vec![[3; 32]];
    let bytes = record.encode().unwrap();
    let decoded = Record::decode(&bytes).unwrap();
    assert_eq!(decoded.encode().unwrap(), bytes);
    let floats = cbor::items(cbor::field(&decoded.body, 0).unwrap()).unwrap();
    for (actual, expected) in floats.iter().zip(values) {
        assert_eq!(
            cbor::f64_value(actual).unwrap().to_bits(),
            expected.to_bits()
        );
    }
    assert!(decoded.check_features(&[1]).is_err());
    assert!(decoded.check_features(&[1, 99]).is_ok());
    assert_eq!(decoded.digest().unwrap(), record.digest().unwrap());
}
#[test]
fn rejects_noncanonical_duplicate_oversized_and_trailing_cbor() {
    for bytes in [
        vec![0x18, 1],
        vec![0xa2, 0, 0, 0, 1],
        vec![0xa2, 1, 0, 0, 0],
        vec![0x9f, 0xff],
        vec![0xf8, 20],
        vec![0, 0],
        vec![0xc0, 0],
        vec![0x9b, 255, 255, 255, 255, 255, 255, 255, 255],
    ] {
        assert!(cbor::decode(&bytes).is_err(), "accepted {bytes:?}");
    }
    assert!(cbor::encode(&Value::Float(f64::INFINITY)).is_err());
    assert!(cbor::encode(&cbor::map([(0, cbor::num(1)), (0, cbor::num(2))])).is_err());
    let mut value = Value::Null;
    for _ in 0..40 {
        value = cbor::array([value]);
    }
    assert!(cbor::encode(&value).is_err());
}
#[test]
fn validates_identity_dependencies_and_reference_shape() {
    let reference = ObjectRef {
        object_id: [1; 16],
        record_id: [2; 16],
    };
    assert_eq!(
        ObjectRef::from_value(&reference.to_value().unwrap()).unwrap(),
        reference
    );
    assert!(
        ObjectRef::from_value(&cbor::array([cbor::bytes([0; 16]), cbor::bytes([1; 16])])).is_err()
    );
    let mut record = Record::new(kind::PAGE, [1; 16], cbor::map([]));
    record.dependencies = vec![[1; 16]];
    assert!(record.encode().is_err());
    record.dependencies = vec![[3; 16], [2; 16]];
    assert!(record.encode().is_err());
}
