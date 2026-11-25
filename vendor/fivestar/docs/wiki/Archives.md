# Archives

## RPF9

Every archive starts with the magic `RPF9`. Only the **table of contents is encrypted**; the entries themselves are
stored plainly.

- Entries are runs of chunks, each starting on a 16-byte boundary. A chunk is a zlib stream (`78 DA`) or stored raw;
  every chunk except the last holds 64 KiB of data.
- Without the table of contents the entries can be recovered, but not their names, and an entry that ends in a raw
  chunk of unknown length cannot be cut exactly.
- `movies.rpf` holds raw Bink 2 videos (515). `shaders.rpf` holds SGD5 shader records. `audio.rpf` holds nested
  archives of raw AWC sound banks rather than zlib streams (see [Audio](#audio)).

Share of each archive that is recovered from zlib streams (sampled):

| Archive | Size | Recovered |
|---|---|---|
| levels.rpf | 70.9 GiB | 91% |
| audio.rpf | 26.6 GiB | 99.9% (raw AWC audio, not zlib) |
| story000.rpf | 18.7 GiB | 31% |
| movies.rpf | 8.2 GiB | 100% (Bink) |
| core_bvh4.rpf | 7.7 GiB | 92% |
| navmeshes.rpf | 6.3 GiB | 95% |
| packs.rpf | 4.5 GiB | 88% |
| anim.rpf | 4.1 GiB | 91% |

## PSO metadata

Most recovered blobs are RAGE PSO files (magic `PSIN`), the binary form of the engine's metadata, or paged
resources (`PRD0`) that embed the same schema. Each file carries its own structure and enum definitions, so every
field's offset and type is known without outside information. Names are joaat hashes; the string tables are encrypted.

| Kind | Count parsed |
|---|---|
| PSO files | 6,026 of 6,026 |
| PRD0 resources | about 39,700 |

The root structure hash identifies the resource type; for example clip dictionaries in `anim.rpf` and navmeshes in
`navmeshes.rpf`. Layout details: `docs/pso.md` in the repository.

## Audio

`audio.rpf` holds 7,160 AWC sound banks with 1.6 million sounds, found without the table of contents. Almost all
are Opus; the rest are XMA2 and PCM. Every sound exports as a playable `.opus` or `.wav` file. Names are not known.
Layout details: `docs/audio.md` in the repository.

## Unknown

- The table-of-contents cipher and key.
- Names of most structure and member hashes.
- The layout of vertex and index data in the older paged resources.
