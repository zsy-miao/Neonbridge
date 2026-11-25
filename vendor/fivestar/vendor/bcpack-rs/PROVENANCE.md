# bcpack-rs test streams

| | |
|---|---|
| Upstream | https://github.com/Ahmet-Dedeler/bcpack-rs |
| Revision | `0a887de1edc9c22a425605091f1a8365fdf74dbb` |
| License | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`); used here under MIT |
| Files | `tests/vectors/synthetic_11.json`, `tests/vectors/synthetic_12.json` |
| Local changes | none |

Each file is a synthetic BCPack BC1 stream made by the upstream encoder: its block count, the stream, the 12 lanes'
symbols and the SHA-256 of the BC1 blocks decoded with the test tables of upstream `tests/roundtrip.rs`.
`tests/FiveStar.Formats.Tests/Bc1DecoderTests.cs` and `benchmarks/FiveStar.Benchmarks/Bc1DecodeBenchmarks.cs` read
them. The decoder's tables and their origin are listed in `THIRD_PARTY_NOTICES.md` at the repository root.
