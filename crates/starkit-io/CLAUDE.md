# starkit-io — crate rules

The only crate that touches files and codecs (INV-3 boundary). Root `CLAUDE.md` applies on top.

- **Single write path:** `atomic_write(path, bytes)` — temp file in the same directory → fsync the file (best-effort fsync of the directory) → rename. Every encoder goes through it. A direct `File::create` anywhere else in the workspace is a review blocker.
- **Inputs are sacred:** input files are never modified; there is no in-place mode, and none may be added.
- **Decode contract (T1-1):** TIFF 8/16 RGB, PNG, JPEG → linear f32 RGB. Apply the embedded ICC transform; when untagged, assume sRGB and record a warning in the result metadata. Capture ICC profile bytes and EXIF **verbatim** for re-embedding.
- **Encode contract:** TIFF16 default; restore ICC + EXIF; de-linearize through the same transform used at decode. FR-1's no-op round-trip test (decode → encode with all operations disabled ⇒ pixel-identical TIFF16) is this crate's north star.
- **ICC backend:** undecided (lcms2 vs qcms vs minimal built-in curves for sRGB/AdobeRGB/ProPhoto) — decide at T1-1, license-check per INV-6, record as D-005 resolution.
- **Error taxonomy:** typed via `thiserror` — `Decode { format }`, `UnsupportedLayout`, `IccTransform`, `Io` — so `starkit-cli` can map them to its stable exit-code table (T1-8).
