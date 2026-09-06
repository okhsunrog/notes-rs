# Known-answer chunks

`two-segments.raw.inkchnk` and `two-segments.pco.inkchnk` encode the same synthetic
five samples, split 3+2 across two segments. IDs are repeated bytes 01 (chunk),
02 (profile), 03/04 (segments). Semantic columns are:

- X F64: +0, -0, 1.25, minimum positive subnormal, -MAX.
- Y F64: 10, 12, 14, 14, 14.
- Pressure U16: valid positions 0,2,4 have 0,1024,4095; bitmap 0x15.
- Tilt X I8: all null.
- Sample kind U8: 0,1,2,0,0.

Generated with ink-format 0.1 and pco 1.0.3 level 8, Auto mode/delta, native dtypes,
equal pages up to 2^18 values, 8-bit numeric types enabled. RAW bytes are frozen.
PCO bytes are decoder fixtures: future encoders need not emit identical streams.
These files contain no user handwriting. Semantic identifiers/dtype/validity/bytes
are all compared on decode, including signed zero and extreme floats.
