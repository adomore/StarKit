# starkit-cli — crate rules

The CLI (`starkit`). Orchestration only: it wires `starkit-io` (decode/encode) to `starkit-core` (algorithms) and owns nothing photometric. Root `CLAUDE.md` applies on top.

- **Errors:** `anyhow` is allowed here (this is the binary). Every failure is still mapped to a value in the stable exit-code table before the process exits.
- **INV-1 holds by construction:** the pipeline never writes an operation's output directly — reduction returns a modified plane and `gate::composite` applies it. A `File::create` or a direct pixel write anywhere in this crate is a review blocker; all image writes go through `starkit-io`.
- **INV-2 through the CLI:** a re-run must produce hash-identical output images (FR-8). No wall clock reaches an output: reports carry no timing, and batch inputs are processed in sorted order so filesystem enumeration cannot reorder them.
- **Reduction runs per channel** with geometry fixed once on the mean-of-RGB plane, so colour survives and the channels stay registered.

## Stable exit-code table (`src/exit.rs`)

Numbers are a contract — add, never renumber. `tests/cli_ac.rs` asserts them through the compiled binary.

| code | name | meaning |
|---|---|---|
| 0 | `OK` | everything succeeded |
| 2 | `USAGE` | bad arguments / preset (clap's own convention) |
| 3 | `IO` | an input could not be read or an output written |
| 4 | `DECODE` | input not decodable (truncated, corrupt, wrong format) |
| 5 | `UNSUPPORTED` | decoded but an unhandled layout, or a mismatched sky mask |
| 6 | `PROCESSING` | a processing step failed on valid data |
| 7 | `BATCH_PARTIAL` | a batch finished but skipped ≥1 image (it did **not** abort) |
| 8 | `BENCH_REGRESSION` | the 61 MP bench exceeded the hard-cap timing budget (T1-9) |
