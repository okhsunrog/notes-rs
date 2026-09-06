# ink-format

Experimental, application-independent Rust library for columnar digital ink.
It lives in this workspace for development but has an explicit package version
and registry-only dependencies. No Tauri, SQLite, device SDK, network, global
configuration, or workspace dependency inheritance is required.

## Implemented

- INKCHNK v1 chunk reader/writer over bytes and `Read`/`Write`.
- All ten integer/float scalar types, full values and mixed/all-null validity.
- Lossless RAW and PCO level 8; no quantization or external compression.
- Stable segment IDs independent of physical chunks; chunk/profile IDs supplied
  by the caller rather than tied to a particular UUID generator.
- Deterministic CBOR metadata envelopes, strong dependency/resource references,
  object/version references and explicit required-feature checks.
- Typed document, page, stroke, geometry, brush, source-profile and session bodies.
- Complete core-ink snapshot validation: reference closure, identities, hashes,
  profile/column agreement, segment mapping, timeline and resource checks.
- Checksums, bounds, counts, duplicate IDs, malformed streams and fixed fixtures.

```rust
use ink_format::{DType, Encoding, chunk::{Chunk, Column, f64_column}};

let chunk = Chunk {
    id: [1; 16],
    profile: [2; 16],
    segments: vec![([3; 16], 2)],
    columns: vec![
        f64_column(1, [10.0, 12.5].into_iter()),
        f64_column(2, [20.0, 21.0].into_iter()),
        Column {
            semantic: 12, // Sample origin: measured, synthetic edit, or unknown import.
            dtype: DType::U8,
            required: true,
            validity: None,
            values: vec![0, 0],
        },
    ],
};
let bytes = chunk.encode(Encoding::Pco8)?;
let decoded = Chunk::decode(&bytes)?;
assert_eq!(decoded, chunk);
# Ok::<(), ink_format::Error>(())
```

See [FORMAT.md](FORMAT.md) for the implemented wire contract and
[fixtures](tests/fixtures/README.md) for known-answer files.
`cargo run --example inspect -- path/to/chunk.inkchnk` checks a chunk and prints
its segment and column counts without a database or application runtime.

## Boundaries

`Record::decode` validates the envelope, not every possible body schema, feature,
brush, or graph. Checking required features is necessary but not sufficient to
render/edit: the consumer must also understand the record kind and body contract.
Unknown CBOR body/extension values and unknown numeric column semantics can be
preserved without interpreting them. Structural parsing is not semantic support.
`model` supplies typed core bodies; `snapshot::Snapshot::validate` checks a whole
core-ink graph. Recognition, corrections, images and text objects remain future
work and the snapshot validator rejects them. Unknown high-numbered body fields
are preserved by extensible typed bodies; nested structures without an extension
map reject unsupported fields rather than silently discarding them.

This crate does not own documents, assign timestamps, acquire stylus input,
render strokes, choose an OCR provider, implement recognition selection policy,
manage an Undo stack, or decide where/when to store data. A database/filesystem
adapter owns transactions, atomic publication, crash recovery, history retention,
compaction scheduling and garbage collection. The codec never chooses chunk size.
A complete portable document container is not implemented yet; a chunk is not a
self-contained note.

No legacy application JSON import is planned: the test drafts will be discarded
when the application switches storage. XOPP conversion should be an independent
adapter and is not part of this initial codec.

## Limits and compatibility

Chunk input and total decoded columns are capped at 64 MiB; one chunk permits
at most 1,000,000 samples, 65,536 segments and 64 columns. Metadata is capped at
16 MiB, depth 32 and 1,000,000 CBOR items. These guard stored/output sizes and
parsing depth; they are not a hard process-wide memory/CPU quota on dependencies.
For each PCO column the declared sample count is checked before allocating its
output, actual count and dtype must match, and trailing stream data is rejected.

Version 0.1 is experimental, with no stable API/MSRV promise. `publish = false`
prevents accidental publication while the format is being validated. Before a
crates.io release: choose a license, stabilize compatibility fixtures and public
API, set/test an MSRV, then enable publication. No license is invented here on
behalf of the repository owner.

The checked-in lockfile of the consuming workspace pins the tested dependencies.
Decoders must continue to read committed fixtures across encoder upgrades; byte
identity of newly compressed output is not required.
