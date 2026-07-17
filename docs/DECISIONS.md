# StarKit — Decision Log

Append-only. One entry per nontrivial choice: context, decision, alternatives considered. Reference the D-number in commits when relevant.

- **D-001** 2026-07-16 — Project name confirmed: **StarKit** (was working title). PRD promoted v0.1 → v1.0 (approved).
- **D-002** 2026-07-16 — Handoff to Claude Code proceeds with Q1/Q2 still open; both are encoded as Gate G0 checklist items in ROADMAP.md. Rationale: fixture/oracle work (T0-1..T0-4) does not depend on their answers.
- **D-003** 2026-07-16 — Crate licenses left unset and `publish = false` everywhere, pending Q5 (private tool vs public release).
- **D-004** 2026-07-16 — Scaffold pinned to Rust edition **2021** for broad toolchain compatibility (verified on rustc 1.75). Claude Code may bump to edition 2024 once the local toolchain is confirmed — record the bump here.
- **D-005** 2026-07-16 — ICC transform backend (lcms2 vs qcms vs minimal built-in sRGB/AdobeRGB/ProPhoto curves) intentionally undecided; decide at T1-1 and record here (license check per INV-6).
