# Mutation testing notes

The first full `cargo-mutants` baseline on 2026-07-16 tested 235 mutants: 131
were caught, 67 were missed, and 37 were unviable. The missed set exposed real
gaps around parser boundaries, builder behavior, formatter types, reserved-field
collisions, duration units, and response-body delegation. Tests were added for
those observable contracts.

Three mutations are excluded narrowly in `.cargo/mutants.toml`:

- Replacing the two custom `Debug::fmt` implementations with `Ok(())` changes
  diagnostic text only. Debug output is intentionally not a stable public
  contract and does not affect request or logging behavior.
- Replacing bitwise OR with XOR in `decode_hex_byte` is equivalent. The decoded
  high nibble occupies bits 4–7 and the low nibble bits 0–3, so those sets never
  overlap and OR and XOR produce the same byte for every valid input.

Do not broaden these exclusions. New surviving mutants require either a
behavioral test or a written equivalence/tool-limitation explanation here.

The final full baseline tested 228 non-excluded mutants: 191 were caught, 37
were unviable, and none were missed.
