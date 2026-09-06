use ink_format::{
    DType, Encoding, cbor,
    chunk::{Chunk, Column, f64_column},
    model::*,
    record::ObjectRef,
    snapshot::{Snapshot, blob_ref},
};
use std::collections::BTreeMap;
fn sample() -> Snapshot {
    let axes = [1, 2, 12]
        .into_iter()
        .map(|id| {
            (
                id,
                Axis {
                    unit: if id == 12 { "kind" } else { "unit" }.into(),
                    representation: if id == 12 { 3 } else { 0 },
                    convention: "example".into(),
                    min: None,
                    max: None,
                    extra: BTreeMap::new(),
                },
            )
        })
        .collect();
    let profile = SourceProfile {
        axes,
        provenance: cbor::map([(0, cbor::text("example-v1"))]),
        tick_ns: None,
        extra: BTreeMap::new(),
    }
    .record([1; 16])
    .unwrap();
    let chunk = Chunk {
        id: [2; 16],
        profile: profile.id,
        segments: vec![([3; 16], 2)],
        columns: vec![
            f64_column(1, [0., -0.].into_iter()),
            f64_column(2, [10., 20.].into_iter()),
            Column {
                semantic: 12,
                dtype: DType::U8,
                required: true,
                validity: None,
                values: vec![0, 0],
            },
        ],
    };
    let geometry = Geometry {
        profile: profile.id,
        timing: None,
        point_count: 2,
        parts: vec![Part {
            segment: [3; 16],
            first_sample: 0,
            count: 2,
            first_t_tick: 0,
        }],
        bounds: None,
        edit_provenance: None,
        extra: BTreeMap::new(),
    }
    .record([4; 16])
    .unwrap();
    let brush = Brush {
        contract: "example-brush".into(),
        version: 1,
        parameters: cbor::map([]),
        extra: BTreeMap::new(),
    }
    .record([5; 16])
    .unwrap();
    let stroke = Stroke {
        id: [6; 16],
        geometry: geometry.id,
        brush: brush.id,
        rgba: [0, 0, 0, 255],
        width: 1.234567890123,
        transform: [1., 0., 0., 1., 0., 0.],
        created_at: Some(i64::MAX),
        updated_at: None,
        extra: BTreeMap::from([(0x8000, cbor::text("preserved"))]),
    }
    .record([7; 16])
    .unwrap();
    let page = Page {
        id: [8; 16],
        width: 1000.,
        height: 1400.,
        objects: vec![ObjectRef {
            object_id: [6; 16],
            record_id: stroke.id,
        }],
        background: Background::plain([255; 4]),
        recognition: vec![],
        corrections: vec![],
        recognition_selection: BTreeMap::new(),
        extra: BTreeMap::new(),
    }
    .record([9; 16])
    .unwrap();
    let records: BTreeMap<_, _> = [profile, geometry, brush, stroke, page]
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
    let bytes = chunk.encode(Encoding::Pco8).unwrap();
    let root = Document {
        id: [10; 16],
        pages: vec![ObjectRef {
            object_id: [8; 16],
            record_id: [9; 16],
        }],
        chunks: BTreeMap::from([(chunk.id, blob_ref(&bytes))]),
        segments: BTreeMap::from([(
            [3; 16],
            SegmentRef {
                chunk: chunk.id,
                index: 0,
            },
        )]),
        records: records
            .iter()
            .map(|(k, v)| (*k, blob_ref(&v.encode().unwrap())))
            .collect(),
        resources: BTreeMap::new(),
        extra: BTreeMap::new(),
    }
    .record([11; 16])
    .unwrap();
    Snapshot {
        root,
        records,
        chunks: BTreeMap::from([(chunk.id, bytes)]),
        resources: BTreeMap::new(),
    }
}
fn reseal(s: &mut Snapshot) {
    let mut root = Document::read(&s.root).unwrap();
    root.records = s
        .records
        .iter()
        .map(|(k, v)| (*k, blob_ref(&v.encode().unwrap())))
        .collect();
    s.root = root.record(s.root.id).unwrap();
}
#[test]
fn complete_snapshot_roundtrips_typed_records() {
    let s = sample();
    s.validate(&[1]).unwrap();
    let stroke = Stroke::read(&s.records[&[7; 16]]).unwrap();
    let restored = Stroke::read(&stroke.record([7; 16]).unwrap()).unwrap();
    assert_eq!(stroke, restored);
    assert_eq!(stroke.width.to_bits(), 1.234567890123f64.to_bits());
    assert_eq!(stroke.created_at, Some(i64::MAX));
}
#[test]
fn rejects_missing_or_corrupted_parts() {
    let mut s = sample();
    s.records.remove(&[4; 16]);
    assert!(s.validate(&[1]).is_err());
    let mut s = sample();
    s.chunks.get_mut(&[2; 16]).unwrap()[100] ^= 1;
    assert!(s.validate(&[1]).is_err());
    let mut s = sample();
    let mut doc = Document::read(&s.root).unwrap();
    doc.segments.get_mut(&[3; 16]).unwrap().index = 9;
    s.root = doc.record(s.root.id).unwrap();
    assert!(s.validate(&[1]).is_err());
}
#[test]
fn rejects_wrong_identity_reference_cycle_and_partial_geometry() {
    let mut s = sample();
    let mut page = Page::read(&s.records[&[9; 16]]).unwrap();
    page.objects[0].object_id = [30; 16];
    s.records.insert([9; 16], page.record([9; 16]).unwrap());
    reseal(&mut s);
    assert!(s.validate(&[1]).is_err());
    let mut s = sample();
    s.records.get_mut(&[1; 16]).unwrap().dependencies = vec![[9; 16]];
    reseal(&mut s);
    assert!(s.validate(&[1]).is_err());
    let s = sample();
    let mut g = Geometry::read(&s.records[&[4; 16]]).unwrap();
    g.parts[0].first_sample = 1;
    assert!(g.record([4; 16]).is_err());
}
#[test]
fn rejects_singular_transform_and_unknown_fields() {
    let s = sample();
    let mut stroke = Stroke::read(&s.records[&[7; 16]]).unwrap();
    stroke.transform = [0.; 6];
    assert!(stroke.to_value().is_err());
    let mut value = s.records[&[7; 16]].body.clone();
    value
        .as_map_mut()
        .unwrap()
        .push((cbor::num(20), cbor::num(1)));
    assert!(Stroke::from_value(&value).is_err());
}

#[test]
fn rejects_channel_dtype_even_with_valid_checksums_and_catalog() {
    let mut s = sample();
    let mut chunk = Chunk::decode(&s.chunks[&[2; 16]]).unwrap();
    chunk.columns[0].dtype = DType::U64;
    chunk.columns[0].values = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
    let bytes = chunk.encode(Encoding::Pco8).unwrap();
    let mut doc = Document::read(&s.root).unwrap();
    doc.chunks.insert(chunk.id, blob_ref(&bytes));
    s.chunks.insert(chunk.id, bytes);
    s.root = doc.record(s.root.id).unwrap();
    assert!(s.validate(&[1]).is_err());
}

#[test]
fn grid_roundtrips_and_plain_rejects_grid_fields() {
    let mut background = Background::plain([255; 4]);
    background.grid = Some(Grid {
        step: [25., 25.],
        origin: [0., 0.],
        line_width: 1.,
        line_rgba: [119, 119, 119, 255],
    });
    assert_eq!(
        Background::from_value(&background.to_value().unwrap()).unwrap(),
        background
    );
    let mut value = Background::plain([255; 4]).to_value().unwrap();
    value
        .as_map_mut()
        .unwrap()
        .push((cbor::num(2), cbor::float(25.)));
    assert!(Background::from_value(&value).is_err());
}
