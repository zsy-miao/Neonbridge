# PSO files and PRD0 resources

PSO ("parser structure objects") is the binary form of the game's metadata. Each file carries its own struct and
enum schemas, so it can be walked into a typed tree without outside knowledge. Names are joaat hashes; the string
tables are encrypted. The same tables also sit inside PRD0 paged resources.

## Framing

A PSO file is a list of chunks: a 4-letter tag, a big-endian u32 size (including the 8-byte header), the payload.

| Chunk | L-format (this game) | Classic (big-endian) |
|---|---|---|
| `PSIN` | Block table and data blocks | Data blocks |
| `LSCH` / `PSCH` | Struct schemas | Schemas |
| `LENM` | Enums | (in `PSCH`) |
| `LSNS` / `STNS` | String namespaces, encrypted | Encrypted |
| `LGID` | Always empty | - |
| `LRSC` / `PMAP` | `PRD0` header with the root block id | Block map and root |
| `CHKS` | BE u32 total file size, u32 checksum, `zppp` | Same |

Payloads of the L-format are little-endian. Fill bytes: `0xAA` runtime pointer slots, `0x70` padding.

### PSIN

| Offset | Size | Meaning |
|---|---|---|
| +0x08 | 16 per block | Block table rows `{u32 type, u32 size, u64 0xAAAAAAAAAAAAAAAA}`; the table ends at the first row without the marker |
| next 16-byte boundary | | Blocks in table order, each 16-byte aligned |

A block's type is a struct name hash, a synthesized struct id (0x100 and up) or a primitive type code. Block ids are
1-based.

### LSCH

`n` struct headers, then all member records. `n` is the count for which `8 + 0x20 * n + 0x10 * (sum of member
counts)` equals the chunk size.

| Struct header | Size | Meaning |
|---|---|---|
| +0x00 | 4 | Name hash |
| +0x04 | 4 | Signature hash |
| +0x08 | 4 | Flags; byte 1 = log2(alignment) |
| +0x0C | 4 | 0 |
| +0x10 | 8 | Runtime slot (`0xAA`; in PRD0 the member-list pointer) |
| +0x18 | 4 | Size |
| +0x1C | 2 | 0 |
| +0x1E | 2 | Member count |

| Member record | Size | Meaning |
|---|---|---|
| +0x00 | 4 | Name hash (0x100 = element descriptor of an array or map) |
| +0x04 | 4 | Offset in the struct |
| +0x08 | 4 | Type: low byte = type code, byte 1 = subtype, high 16 bits = element member index or bit count |
| +0x0C | 4 | Struct hash, enum hash, element count or key/value member indices |

### LENM

`n` headers `{u32 name, u32 signature, u64 runtime, u32 value count, u32 flags}` (0x18 bytes), then all values
`{i64 value, u32 name hash, u32 0}`; `n` solved like `LSCH`.

### LRSC

`PRD0` (bytes `30 44 52 50`), u32 0x7A, 8 zero bytes, u32 root block id at +0x10 of the payload.

## Pointers and containers

| Code | Type | Size |
|---|---|---|
| 0x07 | Pointer: u64, block id = low 20 bits (0 = null), byte offset = the upper 44 bits | 8 |
| 0x5C | Array `{u32 count, u32 capacity, pointer}` | 16 |
| 0x52 | Array `{pointer, u16 count, u16 capacity, padding}` | 16 |
| 0x50, 0x53, 0x54 | Inline fixed array: ref = element count, aux = element member | count x element |
| 0x71, 0x73, 0x75, 0x76, 0x77 | Map: u32 count, then a pointer to a block of node structs | 24 / 16 |
| 0x05 | Inline struct, ref = struct hash | struct size |
| 0x01, 0x10-0x17 | bool, s8, u8, s16, u16, s32, u32, u64, s64 | 1-8 |
| 0x20, 0x21, 0x23 | f16, f32, f64 | 2, 4, 8 |
| 0x30-0x36 | vec2, vec3 (16), vec3 packed (12), vec4, vec3V, 3x4 and 4x4 matrices | 8-64 |
| 0x4A, 0x4B, 0x4D, 0x4F | Hash, hash, namespaced hash, 64-bit namespaced hash | 4, 4, 4, 8 |
| 0x60-0x62 | Enum of 8/16/32 bits, ref = enum hash | 1, 2, 4 |
| 0x63-0x65, 0x68 | Bitset of 8/16/32/64 bits | 1-8 |
| 0x08, 0x0C, 0x0D, 0x40, 0x42, 0x44 | Unknown (string or pointer variants) | 8, 8, 16, 16, 8, 16 |

## PRD0 resources

A virtual segment (pointers 0x50000000 + offset) whose root object is:

| Offset | Size | Meaning |
|---|---|---|
| 0x00 | 8 | 0 (vtable slot) |
| 0x08 | 8 | Pointer to a small page map |
| 0x10 | 16 | `PRD0`, u32 0x7A, u64 0 |
| 0x20 | 4 | Root block id |
| 0x28 | 8 | Pointer to the struct headers (0x20 bytes each, as `LSCH`) |
| 0x30 | 8 | Pointer to the enum headers (0x18 bytes each, as `LENM`) |
| 0x38 | 8 | Pointer to the block table `{u32 type, u32 size, u64 address}` |
| 0x40 | 8 | Pointer to the string namespaces (encrypted) |
| 0x48 | 2, 2, 2 | Struct, enum and block counts |
| 0x60 | | First data block |

A block address with top nibble 6 is in the physical segment. Data inside blocks uses the PSO pointer encoding.

## Classic PSO

`PMAP`: BE u32 root id, u32 0, u16 count at +0x10 of the payload, entries from +0x18:
`{u32 struct hash, u32 file offset, u32 0, u32 length}`.

## Joaat

Names hash with Jenkins one-at-a-time over the lower-cased UTF-8 text.

## Commands

```
fivestar archive pso info   <blob>                    chunks, blocks, root type
fivestar archive pso schema <blob>                    struct, member and enum tables
fivestar archive pso dump   <blob> [--max N] [--chars N]
fivestar archive pso check  <dir|blob> ...            walk every PSO file and PRD0 resource, count errors
fivestar archive pso roots  <dir> ... [--top N]       count blobs per root struct hash
fivestar archive pso hashes <dir> ... [--out F]       harvest hashes per category as JSON
fivestar archive pso crack  <hashes.json> <words.txt> [--out F]
```

`<blob>` is a carved `.psin.bin` or `.prd0.bin`. `dump` prints the root object as JSON: keys are 8-digit hex name
hashes, hashes print as `h:xxxxxxxx`, matched enum values as `enum:xxxxxxxx`, unknown types as `rawNN:hex`,
vector components rounded to 6 decimals; `--max` caps the member values read. `hashes` collects struct names above
0xFFFF, member names, enum names, enum value names and non-zero hash-typed data values. `crack` joaat-hashes each
word against a harvest and writes `hash<TAB>category<TAB>word` lines.

## Unknown

- Map layouts other than 0x75, and types 0x08, 0x0C, 0x0D, 0x40, 0x42, 0x44, 0x76, 0x77.
- The `CHKS` checksum algorithm and the page map of PRD0 resources.
- Walking classic PSO data.
