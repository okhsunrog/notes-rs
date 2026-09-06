use ink_format::{
    DType, Encoding,
    chunk::{Chunk, Column, f64_column, f64_values},
};

fn sample() -> Chunk {
    Chunk {
        id: [1; 16],
        profile: [2; 16],
        segments: vec![([3; 16], 3), ([4; 16], 2)],
        columns: vec![
            f64_column(
                1,
                [0.0, -0.0, 1.25, f64::from_bits(1), -f64::MAX].into_iter(),
            ),
            f64_column(2, [10.0, 12.0, 14.0, 14.0, 14.0].into_iter()),
            Column {
                semantic: 3,
                dtype: DType::U16,
                required: false,
                validity: Some(vec![0b10101]),
                values: [0u16, 1024, 4095]
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            },
            Column {
                semantic: 5,
                dtype: DType::I8,
                required: false,
                validity: Some(vec![0]),
                values: vec![],
            },
            Column {
                semantic: 12,
                dtype: DType::U8,
                required: true,
                validity: None,
                values: vec![0, 1, 2, 0, 0],
            },
        ],
    }
}
#[test]
fn roundtrip_both_codecs_preserves_bits_segments_and_missing_values() {
    for encoding in [Encoding::Raw, Encoding::Pco8] {
        let source = sample();
        let encoded = source.encode(encoding).unwrap();
        assert_eq!(Chunk::decoded_value_bytes(&encoded).unwrap(), 91);
        let decoded = Chunk::decode(&encoded).unwrap();
        assert_eq!(source, decoded);
        assert_eq!(
            f64_values(&decoded.columns[0]).unwrap()[1].to_bits(),
            (-0.0f64).to_bits()
        );
    }
}
#[test]
fn all_scalar_types_roundtrip_with_pco() {
    let mut chunk = sample();
    macro_rules! column {
        ($type:ty,$dtype:ident,$values:expr) => {{
            let values: [$type; 5] = $values;
            chunk.columns.push(Column {
                semantic: 100 + chunk.columns.len() as u16,
                dtype: DType::$dtype,
                required: false,
                validity: None,
                values: values.into_iter().flat_map(<$type>::to_le_bytes).collect(),
            });
        }};
    }
    column!(u8, U8, [0, 1, 128, 254, 255]);
    column!(i8, I8, [-128, -1, 0, 1, 127]);
    column!(u16, U16, [0, 1, 32768, 65534, 65535]);
    column!(i16, I16, [-32768, -1, 0, 1, 32767]);
    column!(u32, U32, [0, 1, 1 << 31, u32::MAX - 1, u32::MAX]);
    column!(i32, I32, [i32::MIN, -1, 0, 1, i32::MAX]);
    column!(u64, U64, [0, 1, 1 << 63, u64::MAX - 1, u64::MAX]);
    column!(i64, I64, [i64::MIN, -1, 0, 1, i64::MAX]);
    column!(
        f32,
        F32,
        [0.0, -0.0, f32::from_bits(1), f32::MAX, -f32::MAX]
    );
    assert_eq!(
        chunk,
        Chunk::decode(&chunk.encode(Encoding::Pco8).unwrap()).unwrap()
    );
}
#[test]
fn rejects_every_truncation_and_checksum_corruption() {
    for encoding in [Encoding::Raw, Encoding::Pco8] {
        let bytes = sample().encode(encoding).unwrap();
        for n in 0..bytes.len() {
            assert!(Chunk::decode(&bytes[..n]).is_err(), "accepted prefix {n}");
            assert!(Chunk::decoded_value_bytes(&bytes[..n]).is_err());
        }
        for i in [16, 80, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[i] ^= 1;
            assert!(Chunk::decode(&bad).is_err());
            assert!(Chunk::decoded_value_bytes(&bad).is_err());
        }
    }
}
#[test]
fn rejects_invalid_writer_inputs_and_incomplete_scalars() {
    let mut chunk = sample();
    chunk.id = [0; 16];
    assert!(chunk.encode(Encoding::Raw).is_err());
    let mut chunk = sample();
    chunk.columns[2].validity = Some(vec![0xff]);
    assert!(chunk.encode(Encoding::Raw).is_err());
    let mut chunk = sample();
    chunk.columns[0].values[0..8].copy_from_slice(&f64::NAN.to_le_bytes());
    assert!(chunk.encode(Encoding::Pco8).is_err());
    let mut chunk = sample();
    chunk.columns.swap(0, 1);
    assert!(chunk.encode(Encoding::Raw).is_err());
    let mut c = sample().columns.remove(0);
    c.values.push(1);
    assert!(f64_values(&c).is_err());
}
fn checksums(bytes: &mut [u8]) {
    let end = 80 + u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
    let h = crc32fast::hash(&bytes[..72]);
    let d = crc32fast::hash(&bytes[80..end]);
    bytes[72..76].copy_from_slice(&h.to_le_bytes());
    bytes[76..80].copy_from_slice(&d.to_le_bytes());
}
#[test]
fn checks_structural_errors_even_with_correct_checksums() {
    let bytes = sample().encode(Encoding::Raw).unwrap();
    let c = 80 + 2 * 32;
    for (offset, value) in [
        (c + 4, 99),
        (c + 6, 2),
        (c + 2, 255),
        (c + 10, 255),
        (c + 16, 1),
    ] {
        let mut bad = bytes.clone();
        bad[offset] = value;
        checksums(&mut bad);
        assert!(Chunk::decode(&bad).is_err());
    }
    let mut bad = bytes.clone();
    bad[80 + 32..80 + 48].copy_from_slice(&[3; 16]);
    checksums(&mut bad);
    assert!(Chunk::decode(&bad).is_err());
}
#[test]
fn bounded_garbage_is_never_a_panic() {
    // Deterministic malformed input corpus; useful with sanitizers or Miri too.
    let mut seed = 0x12345678u32;
    for n in 0..2048 {
        let bytes: Vec<_> = (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        assert!(Chunk::decode(&bytes).is_err());
    }
}
#[test]
fn fixed_raw_fixture_matches_wire_contract() {
    let fixture = include_bytes!("fixtures/two-segments.raw.inkchnk");
    assert_eq!(sample(), Chunk::decode(fixture).unwrap());
    assert_eq!(sample().encode(Encoding::Raw).unwrap(), fixture);
}
#[test]
fn fixed_pco_fixture_decodes_without_requiring_identical_encoder_output() {
    assert_eq!(
        sample(),
        Chunk::decode(include_bytes!("fixtures/two-segments.pco.inkchnk")).unwrap()
    );
}

#[test]
fn pco_rejects_wrong_count_type_and_trailing_data_with_valid_outer_crcs() {
    let original = sample().encode(Encoding::Pco8).unwrap();
    let column = 80 + 2 * 32 + 4 * 64;
    let offset =
        u64::from_le_bytes(original[column + 16..column + 24].try_into().unwrap()) as usize;
    let original_payload = &original[offset..];
    let config = pco::ChunkConfig::default()
        .with_compression_level(8)
        .with_enable_8_bit(true);
    let mut trailing = original_payload.to_vec();
    trailing.push(0);
    let mut short = original_payload.to_vec();
    short.pop();
    for replacement in [
        trailing,
        short,
        pco::standalone::simple_compress(&[0u8; 6], &config).unwrap(),
        pco::standalone::simple_compress(&[0u16; 5], &config).unwrap(),
    ] {
        let mut bytes = original[..offset].to_vec();
        bytes.extend_from_slice(&replacement);
        bytes[column + 24..column + 28].copy_from_slice(&(replacement.len() as u32).to_le_bytes());
        bytes[column + 52..column + 56].copy_from_slice(&(replacement.len() as u32).to_le_bytes());
        bytes[column + 48..column + 52]
            .copy_from_slice(&crc32fast::hash(&replacement).to_le_bytes());
        let start = 80 + u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        let payload_len = bytes.len() - start;
        bytes[64..72].copy_from_slice(&(payload_len as u64).to_le_bytes());
        checksums(&mut bytes);
        assert!(Chunk::decode(&bytes).is_err());
    }
}
#[test]
fn stream_api_roundtrips_and_propagates_io_failure() {
    let chunk = sample();
    let mut bytes = Vec::new();
    chunk.write_to(&mut bytes, Encoding::Pco8).unwrap();
    assert_eq!(Chunk::read_from(bytes.as_slice()).unwrap(), chunk);
    struct Fail;
    impl std::io::Write for Fail {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::PermissionDenied.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(matches!(
        chunk.write_to(Fail, Encoding::Raw),
        Err(ink_format::Error::Io(_))
    ));
}
