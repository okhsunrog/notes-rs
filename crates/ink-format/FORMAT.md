# INKCHNK and metadata envelope — implemented wire contract

Experimental v1, 2026-09-06. This document describes the standalone library's
implemented subset. It is not an application database schema or full notebook
container. Metadata body schemas are a separate contract: parsing an envelope
alone does not validate a complete document or declare a brush supported.

## Chunk layout

Header/SegmentEntry/ColumnEntry ниже сохраняют 80/32/64 bytes исходного draft.
Все числа в framing little-endian, записываются явно, без struct padding/transmute.
Directory начинается на offset 80; содержит сначала все SegmentEntry, затем все
ColumnEntry, затем params/validity blobs. Payload начинается на 80+directory_bytes.
Порядок ColumnEntry строго по semantic_id; дубликаты запрещены. Ranges внутри своих
областей, не пересекаются; допускается только нулевой padding. Suffix запрещён.

Header:

| Offset | Bytes | Поле                              |
| -----: | ----: | --------------------------------- |
|      0 |     8 | magic ASCII INKCHNK + NUL         |
|      8 |     2 | major=1                           |
|     10 |     2 | minor=0                           |
|     12 |     4 | required_flags=0                  |
|     16 |    16 | chunk_id                          |
|     32 |    16 | source_profile_id                 |
|     48 |     4 | point_count >0                    |
|     52 |     4 | segment_count >0                  |
|     56 |     2 | column_count >0                   |
|     58 |     2 | reserved=0                        |
|     60 |     4 | directory_bytes                   |
|     64 |     8 | payload_bytes                     |
|     72 |     4 | header_crc32 по bytes[0..72)      |
|     76 |     4 | directory_crc32 по всей directory |

SegmentEntry: offset 0 bytes16 segment_id; 16 u32 count>0; 20 u32 flags=0;
24 u64 reserved=0. row_start — сумма предыдущих count; sum(count)=point_count.
Segment ID не повторяется в одном chunk. В разных упаковках он может повторяться
с идентичной семантикой, но каталог snapshot выбирает ровно одно размещение.

ColumnEntry:

| Offset | Bytes | Поле                                               |
| -----: | ----: | -------------------------------------------------- |
|      0 |     2 | semantic_id                                        |
|      2 |     1 | logical_dtype                                      |
|      3 |     1 | storage_dtype                                      |
|      4 |     2 | codec_id                                           |
|      6 |     2 | codec_wire_version                                 |
|      8 |     2 | outer_id                                           |
|     10 |     2 | flags                                              |
|     12 |     4 | logical_count = point_count                        |
|     16 |     8 | payload_offset от начала chunk                     |
|     24 |     4 | payload_bytes после outer                          |
|     28 |     4 | decoded_bytes = valid_count\*sizeof(storage_dtype) |
|     32 |     4 | params_offset от начала chunk                      |
|     36 |     4 | params_bytes                                       |
|     40 |     4 | validity_offset от начала chunk                    |
|     44 |     4 | validity_bytes                                     |
|     48 |     4 | payload_crc32 физических bytes                     |
|     52 |     4 | codec_bytes после снятия outer                     |
|     56 |     8 | reserved=0                                         |

Dtype: 1 U8, 2 I8, 3 U16, 4 I16, 5 U32, 6 I32, 7 U64, 8 I64, 9 F32, 10 F64.
Flags bits0..1: 0 all-valid, 1 mixed, 2 all-null, 3 reserved; bit2 constant,
bit3 lossy, bit4 required; остальные 0. X/Y/DT/SAMPLE_KIND имеют required=1.
SOURCE_TIME optional для render, но требует feature 3 и сохраняется при edit/export.

Mixed bitmap raw LSB-first, ceil(N/8), tail bits=0, строго 0<popcount<N.
Codec получает только valid samples по порядку строк. All-valid/all-null не имеют
bitmap. All-null запрещён для обязательных осей; count N>0, valid_count=0,
decoded/codec/payload bytes=0. Для всех отсутствующих ranges offset/length=0/0;
CRC пустого payload=0. Не бывает одновременно constant и all-null.

## Implemented codec mapping

Logical and storage dtype are identical. All ten dtypes in the table are native;
there is no integer widening, float reinterpretation, quantization or resampling.
All floating values must be finite. The sign of zero and finite subnormals are
preserved. Any unimplemented flag, codec/wire version, or outer codec is rejected.

- RAW: codec_id=0, codec_wire_version=1, outer_id=0; exact params bytes `a1 00 f4`
  (deterministic CBOR `{0:false}`). Payload is packed LE scalars; its length equals
  valid_count \* scalar_width. Constant elision and shuffle are not supported.
- PCO: codec_id=2, codec_wire_version=1, outer_id=0; exact params byte `a0`
  (empty CBOR map). Payload is exactly one complete pco standalone stream,
  including its header and terminator. Internal PCO chunks are allowed; their
  total sample count must equal valid_count and each numeric type must match
  storage_dtype. `payload_bytes == codec_bytes`; `decoded_bytes` is the LE value
  byte length. Counts and output budgets are checked before output allocation;
  trailing bytes, a second stream, truncated input, and wrong counts/types fail.
- All-null columns always use RAW with no payload, including in a PCO document.

Initial PCO implementation and fixtures use crate pco 1.0.3. Its stream version
is embedded in the PCO stream; the outer codec_wire_version identifies this
embedding contract, not the crate version or encoder effort. Encoder config:
level 8, mode Auto, delta Auto, equal pages up to 2^18 numbers, numeric 8-bit
support enabled. The level is not needed by the decoder. An upgraded crate may
choose different compressed bytes but must still read the existing fixtures.
Decoders currently support the PCO stream versions accepted by pco 1.0.3; unknown
future versions fail explicitly rather than being interpreted as RAW.

The codec does not choose the number of segments per chunk. A writer should assign
new chunk IDs when re-encoding/repacking changes its bytes. Segment IDs may remain
unchanged only when profile, semantics, dtypes, validity and logical value bits
remain unchanged. Copying a segment is not merging its logical stroke identity.

## Semantic channels

Common IDs: 1 X, 2 Y, 3 pressure, 4 DT, 5 tilt X, 6 tilt Y, 7 rotation,
8 tangential pressure, 9 size major, 10 size minor, 11 source time, 12 sample kind.
The framing parser requires all-valid/required X, Y and SAMPLE_KIND, and all-valid/
required DT if present. SAMPLE_KIND is U8: 0 measured, 1 synthetic edit,
2 imported/unknown. Other numeric semantic IDs are retained without interpretation.
Interpreting their units/ranges or editing them requires a source-profile contract;
framing validity alone does not establish it.

## Metadata envelope

Canonical CBOR map with exactly seven unsigned integer keys:

| Key | Value                                                                          |
| --- | ------------------------------------------------------------------------------ |
| 0   | positive record kind integer                                                   |
| 1   | nonzero opaque record ID, 16 bytes                                             |
| 2   | body map, unsigned integer field keys                                          |
| 3   | sorted unique required-feature integers                                        |
| 4   | extension map, unsigned integer extension keys                                 |
| 5   | sorted unique strong dependency record IDs, nonzero bytes16, no self-reference |
| 6   | sorted unique resource hashes, bytes32                                         |

Record kinds: 1 document, 2 page, 3 stroke, 4 geometry, 5 source profile, 6 session,
7 brush, 8 recognition, 9 text correction. Unknown kinds/required features remain
readable as envelopes; applications must reject unsupported editing/interpretation.
The library's `check_features` checks declared feature IDs only. Body schemas,
reference closure, cycles and correspondence of dependencies to body references
need separate validation before a document can be accepted as a whole.

ObjectRef is `[object_id:bytes16, record_id:bytes16]`, both nonzero. It distinguishes
an object's identity from the identity of its immutable version. The library does
not create global state or decide how IDs are generated.

Encoding follows RFC 8949 core deterministic CBOR: shortest integers/lengths and
floats, encoded-byte lexicographic map-key order, definite lengths, unique keys.
Finite floats stay floats, preserving f64 bits including signed zero; no automatic
integer conversion. Tags, undefined, NaN, infinity and other simple values fail.
Unknown values in body/extensions are retained canonically, without UTF-8
normalization. SHA-256 of the encoded envelope is available via `Record::digest`.
The caller manages hash catalogs, authentication, durability and atomic publication.

## Limits and fixtures

See README for decoder limits and tests/fixtures for fixed RAW/PCO files. CRC uses
CRC-32/ISO-HDLC. Checksums detect corruption; they do not authenticate an author.
No layout relies on native Rust struct padding, native endian, or unsafe transmute.
