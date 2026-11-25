> Français : [ANALYSE_ETPF.md](../../fr/recherche/paquet/ANALYSE_ETPF.md)

# ETPF tables and TTPF blocks

The initial range of each TTPF examined starts with an ETPF header. Across the 57 files of the layout, the first four little-endian 32-bit words follow the same constraints.

| Relative offset | Observation |
| --- | --- |
| 0x00 | Signature `ETPF`. |
| 0x04 | Value 634, that is `0x27a`. Its meaning remains to be established. |
| 0x08 | Value N. At least N 16-byte groups follow the header inside the ETPF range. Their exact structure and role remain to be established. |
| 0x0c | Value S, a multiple of 16, greater than or equal to N × 16. The ETPF range contains at least 16 + S bytes. The meaning of S remains to be established. |

These raw groups and the fingerprint of the whole ETPF range can be examined with Python 3.10 or later.

```powershell
python .\inspect_etpf.py .\data\ttdb\ui\platform.ttpf
python .\inspect_etpf.py .\data\ttdb\graphics\ttdb_graphics.ttpf --debut 910 --limite 3
```

For `platform.ttpf`, the range occupies offsets 256 to 4351 inclusive. N is 1 and S is 16. The 16-byte group starts at offset 272 and contains `ffc37f00110320000100000000000000`. The SHA-256 fingerprint of the full range is `eaf3e96c3e305a2715654bb9bfb24b8a37c3ea1868ce1ba68d8af4b1b1ae51fc`. The bytes after this group include extra data and padding. Their boundaries and meaning have not been established.

For `ttdb_graphics.ttpf`, N is 912 and S is 17040. The full ETPF range occupies 49152 bytes. The last group, number 911, starts at file offset 27136.

## Sorted table and candidate index limit

In each of the 57 files, the N 16-byte rows are immediately followed by N **strictly increasing** little-endian 64-bit values. This sequence starts at relative offset `16 + 16 × N` from the ETPF signature and ends at `16 + 24 × N` exclusive. Their role is unknown: their order does not prove a match with the ETPF rows of the same number.

The relative position `16 + 32 × N + S` lies inside all 57 ETPF ranges: in each file examined, all bytes between this position and the end of the range are zero, and the byte before it is non-zero. This describes a candidate bound for the index data area, not the structure of the bytes between the sorted table and this bound. In 55 files, the size of the range matches this bound rounded up to the next multiple of 4,096; two ranges have more padding zeros. The rounding formula therefore does not hold for all files.

For `ttdb_graphics.ttpf`, the sorted table of 912 values occupies file offsets **27,152 to 34,448 exclusive**; its SHA-256 is `a8cb17ad49574247d8735b26aa967daf5b89bcc20776c3b700de183881ac879b`. The candidate bound is at file offset 58,784; the remaining 2,912 bytes up to the end of the range are zero. `inspect_etpf.py` displays these bounds, the table fingerprint and the selected values separately from the ETPF rows.

### Candidate link with rows of the same rank

`python .\comparer_cles_etpf.py . --comparer-blocs` compares the values across the 57 files and, for each repetition, the ETPF rows of the same rank as the value in their respective table. The 357,115 values include 357,067 distinct values: **48 values appear twice**, each in two different files.

For these 48 pairs, the first two words of the rows of the same rank are identical in **46 cases**, the total logical sizes in **46 cases** and the block count in **47 cases**. The total physical sizes are identical in 22 cases; in these 22 cases, all stored bytes, including any padding, are identical.

For the 42 pairs whose blocks are all marked zlib and respect the 64 MiB per block limit, the command checks the end and size of each stream, then compares the SHA-256 fingerprints of the concatenations of decompressed data. **39 pairs have the same fingerprint and three have different fingerprints.** The other six pairs are not compared after decompression by this method. The three differences and their fingerprints are listed in the JSON output.

These results are a strong hint that the sorted value and the row of the same rank are associated, but they show that a repeated value does not guarantee identical content. The meaning of the value itself and the cause of the differences remain unknown.

In the case `0x218e33c9913f242c`, rows 705 of `ttdb_interiors_0_3.ttpf` and 2,272 of `ttdb_maps_4_7.ttpf` have the same first two words and four zlib blocks with logical sizes of 65,536, 65,536, 262,144 and 1,048,576 bytes respectively. The fingerprints of the corresponding blocks are nevertheless different for each of the four blocks. Even identical metadata and sizes are therefore not enough to conclude that the data are interchangeable.

A local search in the candidate table is possible with:

```powershell
python .\inspect_etpf.py .\data\ttdb\interiors\ttdb_interiors_0_3.ttpf --cle 0x30774179a7df53c0 --limite 0
```

The result finds the value at rank 993 and shows the candidate ETPF row of the same rank, associated with blocks 3,602 to 3,605. The search is limited to the file provided; the tool checks that the table is strictly sorted before using a binary search. The result field keeps the word "candidate" because the meaning of the value and the exceptions between files remain open.

## Match with blocks

For each 16-byte group, read its third word as a little-endian 32-bit integer and compute `page = word & 0x1fffff`. The matching logical position in bytes is `page * 4096`. Across the 357,115 groups of the 57 files, all these positions match a start bound of a data block of the RDS5 table. In each file, the positions are strictly increasing and the first one matches block 1.

This match makes it possible to partition the data blocks into successive groups. For an ETPF group, its first block is the one that starts at its logical position. The end bound is the first block of the next ETPF group or, for the last group, the end bound of the logical file. This is a structural association verified on this copy. The business meaning of the groups and their other fields remain to be established.

In `ttdb_graphics.ttpf`, the first pages are 12, 13, 15, 16, 17, 18, 19 and 21, and they match data blocks 1 to 8 respectively. In `ttdb_vehicles.ttpf`, the first pages are 11, 183, 535 and 578, matching blocks 1, 5, 9 and 13. The reader `inspect_etpf.py` now displays the first block, the exclusive end bound and the block count of each group.

## Candidate dimensions

The first little-endian word `x` of each group has two fields compatible with dimensions. The candidate width is `(x & 0x3fff) + 1`, and the candidate height is `((x >> 14) & 0x3fff) + 1`. The four high bits remain distinct. Across the 357,115 groups, both candidate dimensions are always powers of two. The most frequent pairs are 1024 × 1024 for 97,910 groups, 512 × 512 for 80,975 and 256 × 256 for 58,442.

In the 832 groups whose second word is `0x012000e0` and that have a single block, the logical size of this block matches exactly `width × height × 4` bytes rounded up to the next multiple of 4096. For example, the first group of `ttdb_graphics.ttpf` gives 32 × 32 and a block of 4096 bytes. Its decompression does produce 4096 bytes. This correlation supports the interpretation as dimensions and four-byte pixels for this subset. The channel order, the pixel layout and the exact meaning of the second word remain to be verified.

The reader displays these two values as `largeur_candidate` and `hauteur_candidate`, and the four high bits under `bits_28_31`. The 357,115 groups split notably into 60,873 groups with one block, 89,116 with two blocks, 41,968 with three blocks and 164,939 with four blocks.

In **162,375 of the 164,939 four-block groups**, the logical size of the third block is exactly four times that of the second, and that of the fourth is four times that of the third. Among these 162,375 groups, the size of the first block is smaller than that of the second in 132,326 cases and equal in 30,049; it is never greater. The three most frequent profiles, in units of 4,096 bytes, are `3, 8, 32, 128` (56,051 groups), `1, 2, 8, 32` (43,695) and `16, 16, 64, 256` (22,985).

This progression is compatible with an organization in successive resolution levels, but the sizes alone prove neither that the blocks are mipmaps, nor what the first block contains. The other 2,564 four-block groups do not follow the exact rule. `stats_etpf.py` provides the counts and one example per profile; for example, row 0 of `data/ttdb/interiors/ttdb_interiors_0_3.ttpf` starts at block 1 and follows the profile `3, 8, 32, 128`.

The size of the last block is also tied to the candidate surface. For the **41,087 four-block groups** whose second word is `0x0220023b`, it always matches half of the surface, rounded to 4,096 bytes; this is also the case for the **24,265 groups** with word `0x02200243`. For the **13,627 groups** with word `0x00200313`, it always matches one times the surface, rounded. These counts include all four-block groups of each category; they are independent of the filter on the progression by four. Combined with this progression, these relations support the hypothesis of spatial scales, without identifying the graphics format or the content of the blocks.

For three-block groups, the last one is exactly four times larger than the second to last in **41,967 cases out of 41,968**. For two-block groups, this relation holds in only 1,902 cases out of 89,116. In the 41,968 three-block groups, the first is smaller than the second; for two-block groups, it is smaller in 86,835 cases and equal in 2,281. These results call for a separate analysis of the first blocks rather than a uniform rule for all groups.

The only three-block group that does not follow the factor four is row 9,252 of `data/ttdb/props/ttdb_props_3_3.ttpf`: its candidate dimensions are 1,024 × 16 and the logical sizes of its three blocks are 4, 8 and 16 pages of 4,096 bytes. The last block is therefore twice the second to last. The elongated shape of these dimensions is a lead, with no proof that it explains the exception.

### Comparison of candidate levels

In the category `0x002000e3`, 153 groups with candidate dimensions 512 × 512 have four zlib-marked blocks and last three blocks of 65,536, 262,144 and 1,048,576 bytes. `python .\comparer_niveaux_etpf.py .` requires NumPy and picks 15 of them with seed 42. The script assumes four bytes per pixel and 128 × 128 Morton tiles. For each of the two pairs of successive blocks, it reduces the larger one by 2 × 2 averaging and compares the first three bytes of each pixel with those of the smaller one. The control performs the same comparison after a vertical shift of half of the candidate image.

In the **30 comparisons** of this sample, the mean difference without shift is lower than that of the shifted control. With `--nombre 153`, the test covers the **153 candidate groups** of the copy: the result is identical for the **306 comparisons**. This is an additional hint of spatially related content between the blocks, compatible with resolution levels. It does not confirm the Morton layout, the channel order or the exact reduction method: a smooth or repetitive texture can also influence the result, and the test covers only one category.

The first block of each of these 153 groups decompresses to 65,536 bytes. The bytes outside the following five ranges are zero in all these blocks: `[2048, 2112)`, `[4096, 4352)`, `[8192, 9216)`, `[16384, 20480)` and `[32768, 49152)`. The sizes of these ranges match 4 × 4, 8 × 8, 16 × 16, 32 × 32 and 64 × 64 four-byte pixels respectively. This does not mean that all bytes of these ranges are non-zero.

With the same pixel and Morton traversal assumptions, the script also compares these five ranges with each other and the last one with the 128 × 128 candidate image of the next block. For the 153 groups, the comparison without shift gives a lower difference than the shifted control in **153 cases out of 153** for the pairs 32 → 64 and 64 → 128, and in **151 cases out of 153** for 16 → 32. The results are less clear at the smallest dimensions: 128 out of 153 for 8 → 16, and 127 out of 153 for 4 → 8, with two ties. These observations strengthen the hypothesis of small resolutions grouped in the first block, without confirming the exact format or layout.

The option `--nombre 153 --comparer-agencements` also compares 16 tile hypotheses, linear or Morton, with sides from 4 to 512, on the candidate blocks 128, 256 and 512. For each group, the score is the mean of the two differences after 2 × 2 reduction; a lower score indicates a better match between the candidate resolutions. Morton tiles of side 128 get the lowest score for **142 groups out of 153**. The other 11 have a better score with a linear variant: eight at 512, two at 256 and one at 32. The mean of the scores is about 16.28 for Morton 128 and 16.67 for linear 512. This comparison checks only the hypotheses included and is not a reference image: it does not establish the exact layout on its own.

In the 11 cases where Morton 128 does not get the lowest score, the gap between its score and the best goes from about 0.009 to 0.508, with a median of 0.085. The 153 rows all have the same first, second and fourth ETPF words (`0x507fc1ff`, `0x002000e3`, `0x00000000`); the third contains a different start position. These fields therefore do not distinguish the 11 exceptions in this set. The small gaps do not allow us to claim that they really use another layout.

An additional reproducible sample of TTPF blocks was drawn with Python, `random.seed(42)` and up to 40 blocks per file in local traversal order. The 2,160 blocks kept had stored and logical sizes less than or equal to 2,000,000 bytes. In this sample, the 1,889 blocks whose bit 29 is set produced a complete zlib stream at the expected logical size. The remaining 271 blocks were not decoded by zlib. The format of these 271 blocks remains unknown, and the 2,160 trials represent a sample.

A raw block LZ4 trial with 0 to 255 bytes removed at the end of the block did not decode blocks number 1150 of `ttdb_graphics.ttpf` and number 6 of `ttdb_vehicles.ttpf`. This result covers these two samples and this decoding method.

## Size comparison of single-block groups

The script `python .\stats_etpf.py .` walks the 57 TTPF of this folder and compares, for each group associated with a single block, its logical size with several multiples of the candidate surface, rounded to 4,096 bytes. It groups the results by the second word of the ETPF row. Across the 357,115 groups examined:

| Second word | Single-block groups | Match with logical size |
| --- | ---: | --- |
| `0x00200239` | 5,490 | 5,480 match a rounded half surface. |
| `0x00200241` | 1,218 | 1,217 match a rounded half surface. |
| `0x00200311` | 2,654 | 2,652 match a rounded surface. |
| `0x012001b0` | 733 | All 733 match twice the rounded surface. |
| `0x002001b1` | 213 | All 213 match twice the rounded surface. |
| `0x002000e1` | 772 | All 772 match four times the rounded surface. |

These correlations suggest that the second word distinguishes data categories with different sizes per surface. They do not determine a pixel format, a codec or the meaning of the bits on their own. The rounding can make the same small block match several multiples: the categories are therefore not exclusive. The exceptions and the multi-block groups need a separate analysis.

Example: row 485 of `data/ttdb/graphics/ttdb_graphics.ttpf` carries the word `0x00200239`, the candidate dimensions 1,024 × 1,024 and matches the single block 780, whose logical size is 524,288 bytes. The statistics script provides up to three examples per word value to ease checks.

### Hypothesis on the exceptions

The candidate dimensions of the ten exceptions for `0x00200239` are all narrow: five are 4 × 2,048 and five are 256 × 1,024. The only exception for `0x00200241` is 256 × 1,024. In the set of single-block groups of both categories, the logical sizes all match `ceil4096(max(width, 512) × height / 2)`: **5,490/5,490** and **1,218/1,218** respectively. For example, row 1,183 of `data/ttdb/peds/ttdb_peds_0_3.ttpf` gives 4 × 2,048, a logical block of 524,288 bytes and this formula does predict 512 × 2,048 / 2 = 524,288.

The two exceptions for `0x00200311` have candidate heights of 128 and 64. Across the **2,654 single-block groups** carrying this word, `ceil4096(width × max(height, 256))` matches all logical sizes.

`ceil4096` rounds up to the integer multiple of 4,096 greater than or equal to the value. These formulas describe the sizes observed in these subsets; they do not prove a hardware alignment, a specific graphics format or the byte layout. Other formulas may also fit: the script compares several minima and returns the exceptions to the formula without a minimum to allow their examination.

Blocks 4051 of `ttdb_peds_0_3.ttpf` and 5477 of `ttdb_ui.ttpf`, matching two exceptions, are marked by bit 29 and decompress completely in zlib, to 524,288 and 262,144 bytes respectively. The SHA-256 fingerprints of the decompressed data are `52c79048e8ab11d010c70a103b0426ccfc4b1973ef7f4987435b3f9931533794` and `542744f291a7a0dadef72723df00997b24acb80f8397201bcc93a7460f947ac1`. The size validation does not reveal the layout of the content.

A new reproducible sample, `python .\inspect_zlib_padding.py .`, selects with seed 42 up to 40 blocks marked by bit 29 per file among those whose physical and logical sizes do not exceed 2,000,000 bytes. The 2,159 blocks selected decode to the expected logical size. For 2,152 of them, the bytes remaining after the stream are all zero; the other seven have no bytes after the stream. This result is limited to this sample and does not prove that the padding is always zero. This sample is distinct from the one covering the 2,160 blocks of all types cited above.

Bit 29 is not determined by the second ETPF word alone: among the single-block groups of type `0x00200239`, it is set 2,357 times and clear 3,133 times. This is a flag count over the whole of these groups; zlib decoding remains validated by sampling, not on all blocks.

Across the 57 files, 108,610 data blocks have bit 29 clear. Their bounded physical size is smaller than the logical size for 108,521 blocks, equal for 46 and greater for 43. The physical bounds may include padding. For most of these blocks, a simple copy of the stored bytes could therefore not produce the announced logical size; the codec or transformation needed remains unknown. `stats_etpf.py` reproduces these counts by walking all the tables, without decoding these blocks.

Reading the first two bytes of each of these 108,610 blocks gives 197 distinct prefixes; **102,802** have `de` as second byte. The most frequent prefixes are `32de` (27,705), `31de` (22,165) and `33de` (15,860). To reproduce this survey, add `--prefixes` to `stats_etpf.py`. The repetition suggests a common structure, without identifying the codec: 5,808 blocks have another second byte and the prefixes alone validate no decompression method.

The survey by second ETPF word shows several families. All blocks without bit 29 of the groups whose word is `0x0220023b` (47,691 blocks), `0x0120023b` (20,373) or `0x0320023b` (11,677) have `de` in second position. In contrast, among the 1,812 blocks without bit 29 of the groups `0x00200313`, 1,751 have `30` in second position and 51 have `31`. These counts include groups with one or several blocks. They indicate a relation between the ETPF field and the stored bytes; they do not establish whether these families use the same codec or whether their data represent pixels.

### Bits 24 to 29 of the RDS5 table

Across all data blocks, the value `(RDS5_word >> 24) & 0x3f` takes only eight values. Their relation with the second physical byte is particularly clear:

| Bits 24 to 29 | Block count | Second byte observed |
| --- | ---: | --- |
| `0x20` | 918,223 | Not surveyed by this prefix analysis. Bit 29 is set; the zlib trials described above are sampled. |
| `0x04` | 102,725 | `de` for all 102,725. |
| `0x10` | 77 | `de` for all 77. |
| `0x14` | 333 | `db` for all 333. |
| `0x0c` | 35 | `db` for all 35. |
| `0x1c` | 5,167 | Several values, mostly `30` (2,633) and `31` (2,003). |
| `0x18` | 268 | Several values, mostly `31` (132) and `30` (67). |
| `0x00` | 5 | Several values. |

The 1,026,833 blocks counted here exclude the 57 initial ETPF ranges. The six bits are a candidate classification of the stored data, but their precise interpretation remains unknown. The byte match is an observation on this copy, not a proof of a codec. The command `python .\stats_etpf.py . --prefixes` reproduces this table and the full distributions.

To guide the search, the Microsoft documentation states that DirectStorage on Xbox Series supports several stages, including DEFLATE, BCPack and data rearrangements ([overview](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/console/storage/directstorage/directstorage-overview), [BCPack modes](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/dstorage/enums/dstorage_bcpack_mode)). BCPack is a lead to examine for texture data, not an identification of these blocks: no BCPack decoding has been verified on this copy and the prefixes alone do not demonstrate its use.

## Pixel layout trial

`python .\comparer_agencements_etpf.py .` selects with seed 42 up to five groups per file with candidate dimensions 32 × 32, second word `0x012000e0`, a single zlib block and a logical size of 4,096 bytes. On this copy, this gives seven blocks. The script treats the data as four bytes per pixel and computes, for the first three bytes of each pixel, the mean absolute difference between horizontal and vertical neighbors. It compares a linear traversal and a Morton traversal in square tiles of 4, 8, 16 or 32 pixels; a smaller score indicates more similar neighbors for this measure.

For six of the seven blocks, the Morton 16 and Morton 32 variants tie for the best score. For the last one, the linear 32 variant is the best. The mean scores are about 11.54 for Morton 16 or 32, against 17.01 for linear 32. For a 32 × 32 image, the two Morton variants 16 and 32 produce the same order here; this trial therefore does not distinguish them.

With `--largeur 64 --hauteur 64`, the sample contains 24 blocks: Morton 32 and 64 tie for the best score on 14 blocks, Morton 16 on two, Morton 8 on three and linear 4 on five. With `--largeur 128 --hauteur 128`, the sample contains 18 blocks: Morton 64 and 128 tie for the best score on seven blocks, Morton 32 on four, Morton 4 on one, linear 32 on three and linear 4 on three. At each square size, the two largest Morton tiles tested produce the same order in this implementation. The continuity measure and these small samples prove neither the real channel order, nor the pixel type, nor a general layout. They justify testing these layouts on resources whose reference image is known.
