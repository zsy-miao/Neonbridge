# Textures and BCPack

Textures live in `.ttpf` page files (magic `RDS5`) outside the archives: a page table, then an `ETPF` section of
texture descriptors and name hashes. Pages are stored raw, zlib-compressed, or BCPack-compressed. Texture memory uses
the console's 4 KiB and 64 KiB tiled layouts; the exporter undoes the tiling and writes DDS. Layout details:
`docs/textures.md` in the repository.

| Result (all 57 `.ttpf` files) | With the BCPack tables | Without |
|---|---|---|
| Mip 0 decodes exactly | 357,115 of 357,115 | 354,998 |
| Decodes approximately (BC4, BC5) | 0 | 132 |
| Blocked by BCPack pages of BC3, BC6H, BC7 | 0 | 1,985 (1,870 BC7, 98 BC6H, 17 BC3) |

The BCPack tables are not part of the project: you supply `bcpack-tables.bin` yourself (see
[The BCPack tables](#the-bcpack-tables)).

## BCPack

BCPack is the Xbox Series hardware texture codec: an entropy coder for BCn data that separates colour endpoints from
indices and codes them with rANS. The page codec number equals `DSTORAGE_BCPACK_MODE` (1 = BC1 ... 7 = BC7).

| Codec | Pages | Stored | With the tables | Without |
|---|---|---|---|---|
| 1 (BC1) | 102,725 | 10.42 GB | exact | fitted tables (below) |
| 7 (BC7) | 5,167 | 1.07 GB | exact | no decoder |
| 6 (BC6H) | 268 | 0.07 GB | exact | no decoder |
| 3 (BC3) | 35 | 0.01 GB | exact | no decoder |
| 4, 5 (BC4, BC5) | 77, 333 | 0.06 GB | exact | approximate (fitted tables) |

### The BCPack tables

Above the entropy layer, every codec's block layer needs constant tables: a 255-entry dictionary per codec that back
references before the first block read, index rank tables, and the BC7 control words. They are Microsoft's format
data and cannot be derived from the dump, so Five Star Recomp does not ship them in any form. The mechanism follows
[VIctory](https://github.com/marvisak/victory) (GPL-3.0), whose repository carries the file; its licence does not cover
the tables.

```
fivestar setup <dump> --bcpack-tables <path to bcpack-tables.bin>
```

checks the file (77,982 bytes and a SHA-256) and copies it into the work folder; `fivestar run` passes it to the loader
and the `textures` step uses it. The `fivestar texture` commands take `--bcpack-tables <file>` or
`FIVESTAR_BCPACK_TABLES`. Without the file everything works as before: BC1 with our fitted tables, BC4 and BC5
approximately, BC3, BC6H and BC7 as placeholder blocks in the loader and skipped in the export.

With the tables all 108,605 BCPack pages of the dump decode with valid blocks (`fivestar texture bcpack-scan --codec
N`), and exported BC3, BC6H and BC7 textures look right. Speed per core is lower than the BC1-only path: about 37
MB/s for BC1, 25 for BC7.

The tables also settle the BC1 fits below: the rANS models and the 2x2 codebook equal ours, but the dictionary differs
from our preset window in 54 of 255 slots (by up to 3 in a field). 13,426 BC1 pages decode differently; in the first
3,000 of them the changed blocks hold 489,862 3-colour blocks with a transparent pixel under our window and 1,671
under the tables, so the table output is the right one.

### The BC1 decoder

`src/FiveStar.Formats/Ttpf/BcPack` implements the decoder of
[bcpack-rs](https://github.com/Ahmet-Dedeler/bcpack-rs) (revision `0a887de1`, MIT OR Apache-2.0, used under MIT).
bcpack-rs found the format: its lanes are the decoder module of Microsoft's
[silicon-rans](https://github.com/microsoft/silicon-rans) (`mdu_minirans.v`, MIT), whose model tables it copies
exactly, and it fitted the BC1 preset window and index codebook to retail data. The same lane decoder is printed in
patent US11234023B2. bcpack-rs credits SixNG, Xbox2Windows and ttpf-viewer notes for earlier findings. The licence
texts are in `THIRD_PARTY_NOTICES.md`.

The format is in `docs/textures.md`: 12 rANS lanes share one byte stream through a feeder that hands out up to 8
bytes per clock in lane order, and each block is one symbol per lane (a back-reference distance, six endpoint
residuals, four 2x2 index groups). Our decoder simulates the lanes clock by clock, as bcpack-rs does, with three
engines: AVX-512 (all lanes in one 16-lane vector, kept in registers), AVX2 (two 8-lane vectors) and scalar.

### Checks

On the full dump:

| Check | Result |
|---|---|
| Convergence | all 102,725 BC1 pages in 57 files converge (every lane ends legally, only zero padding follows); none reaches an unfitted window slot |
| Engines | AVX-512, AVX2 and scalar give the same blocks (SHA-256 per page) on all 102,725 pages |
| Reference | the bcpack-rs Python decoder, given the same tables, gives the same bytes on 828 sampled pages |
| Mip consistency | 400 random BC1 textures with a BCPack mip 0 and a zlib mip 1: mip 0 downsampled 2x against mip 1, RGB PSNR median 35.8 dB (p10 30.8, p90 41.4). Textures stored as zlib in both mips: median 36.7 dB |
| Extraction | `fivestar texture extract` reads all 354,520 exportable textures, BC1 BCPack pages included |

`fivestar texture bcpack-scan` repeats the first two checks. The tests check the model tables against their published
checksum, the vectorised inverse against the hardware inverse for every model and input, the synthetic streams of
bcpack-rs on every engine, and the engines against each other on damaged streams.

Speed on a Ryzen 9 7950X3D (`fivestar texture bcpack-bench`, one core, BC1 output, MB = 10^6 bytes): AVX-512 about
260 MB/s, AVX2 about 175 MB/s, scalar about 50 MB/s, at about 1.2 decoder clocks per block. A scan of all BC1 pages
(15.3 GB of BC1 from 11.2 GB stored, file reads included) takes about 3.7 s on 32 threads. bcpack-rs reports 34 to 40
MB/s per core.

### Table corrections

The entropy tables are exact; the BC1 preset window and codebook are fits, used only without the user's BCPack tables. Ours differ from bcpack-rs in 26 window
slots and two codebook pairs. They change only block colours, never convergence.

**Cause.** Some upstream window values are off by one. Many pages are one long reference chain from a single window
slot, so a slot whose G1 is one too high (or G0 one too low) lowers c0 - c1 by one step in every block of the chain.
Near-flat blocks then flip to c0 <= c1, the 3-colour mode, and their index 3 becomes transparent black specks. Plain
mip 1 has none of these blocks (0 in 2.86 million); the decoded mip 0 had 1,420 in the 400-texture set, all in chains
rooted at window slots.

**Method.** Three disjoint random sets of BC1 textures (side up to 1024) with a BCPack mip 0 and a plain mip 1: A =
400, B = 600, C = 1,200. The lane symbols give each block's reference chain and its root slot, so a slot value can be
changed without decoding again. Score: each 2x2 quadrant of mip 0 against its mip-1 pixel (RGB squared error).

- **Slot 4** (left empty upstream; 28 pages reach it): a joint fit over the 124,047 blocks that reach it, each page
  scored against the neighbouring mips. Value `[12, 27, 13, 6, 12, 6]`; 12 of 25 per-page fits give exactly this
  value, the others differ by 1 in one or two fields.
- **Other slots**: every field of the value within ±1 (729 candidates), moving the centre to the best total until it
  is stable. A new value is kept when it is cross-validated (fitted without a set, it lowers that set's error, for two
  sets of 200 blocks or more; 9 slots), or when it removes the specks (3-colour blocks with some index-3 pixels drop by
  at least 20 and to at most 20%, no set gains any and no set of 200 blocks or more loses over 0.3 dB; 16 slots).
- **Slot 0** is never read: a reference before block 0 reaches slots 1 to 255.
- **Codebook**: a 2x2 mean cannot see the arrangement inside a quadrant, so mirror pairs were tested by smoothness
  (squared differences across a quadrant's inner edges and to its neighbours) with the table against the two patterns
  swapped. Of 1,230 same-multiset pairs the table wins on all but two in all three sets; those two pairs, symbols
  229/235 and 247/248, are swapped (the second wins on two of three sets and ties on the third).

Changed slots: 3, 4, 22, 27, 33, 46, 69, 72, 76, 80, 81, 96, 102, 105, 108, 110, 112, 115, 117, 120, 123, 132, 133,
165, 196, 197. Each changed entry in `bc1_window.json` and `bc1_quad_table.json` keeps the upstream value and the rule
that accepted it.

| Set | Median PSNR | p10 | Below 25 dB | Blocks with c0 = c1 and index 3 | 3-colour blocks with some index 3 |
|---|---|---|---|---|---|
| A (400) | 35.77 → 35.83 | 30.61 → 30.79 | 4 → 2 | 1,420 → 0 | 5,161 → 1,148 |
| B (600) | 35.56 → 35.64 | 30.56 → 30.97 | 9 → 4 | 68 → 0 | 4,830 → 550 |
| C (1,200) | 35.31 → 35.39 | 30.46 → 30.78 | 9 → 5 | 108 → 0 | 4,032 → 1,041 |
| 28 slot-4 pages | 23.37 → 30.97 | 6.65 → 25.91 | 22 → 3 | | |

No texture got more than 0.19 dB worse. All three sets were also used for fitting.

### Earlier readings of the header

Before the decoder, the start of a page was read as a fixed header. The decoder explains those findings:

| Earlier reading | Now |
|---|---|
| 48-byte header, 6 rows of 8 lanes | the feeder's first clocks: 8 bytes per clock in lane order give lanes 0-7 their header bytes, then the four bytes that load the state; lanes 8-11 wait for free slots |
| budget byte = refills + 3 | header byte 1 = payload length - 1; the payload includes the four state bytes |
| near-constant models 0xDB-0xDE | header byte 0 fields (alphabet narrowing, flush, q); the frequencies come from the model tables |
| twin lanes read equal first bytes | lanes with the same header and symbols code the same bytes |
| lanes continue past their budget, with headers later in the stream | a lane codes several fragments, each with its own two- or three-byte header |

### Open

- The table-driven block layer is about 5 times slower than the BC1-only path; it is not vectorised.
- Without the user's tables BC3, BC6H and BC7 stay undecoded, and the fitted BC1, BC4 and BC5 tables are
  approximations.
