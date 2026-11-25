# XMA2

XMA2 is the Xbox form of WMA Pro: a stream of 2 KiB packets carrying MDCT frames of 512 samples per channel. 1,818
of the game's sounds are XMA2 (all mono, 32 kHz; [audio.md](audio.md)). `src/FiveStar.Formats/Audio/Xma` decodes one
stream of one or two channels; the console decodes it in the SHAPE XMA hardware ([audio-runtime.md](audio-runtime.md)).

```mermaid
flowchart LR
  packet["2 KiB packet"] --> header["32-bit header"]
  header -->|"first frame offset"| carry["finish the frame begun in the last packet"]
  carry --> frames["frames: 15-bit length prefix, end with a more-frames bit"]
  frames --> tiling["tiling: subframes of 512, 256 or 128 per channel"]
  tiling --> sub["subframe: stereo transform, quantisation, scale factors, coefficients"]
  sub --> imdct["dequantise per band, inverse MDCT"]
  imdct --> overlap["sine-window overlap with the previous block"]
  overlap --> pcm["512 samples per channel"]
```

## Packet

Bits are read most significant first.

| Bits | Field |
|---|---|
| 6 | Frame count; summed over a stream it equals the frames decoded |
| 15 | First frame offset: bits after the header that finish the frame begun in an earlier packet |
| 3 | Metadata (1 in every packet of the dump; unused by the decoder) |
| 8 | Packet skip: packets of other streams before this stream's next one (0 in all 49,956 packets of the dump) |

The rest is frame data. A frame length of 0x7FFF, or a frame whose last bit is 0, ends the packet's frames; a frame
longer than the rest of the packet continues in the next one.

## Frame

XMA2 fixes the WMA Pro decode flags at 0x10D6: every frame starts with its length, carries a dynamic range byte, and
splits into at most four subframes per channel.

| Bits | Field |
|---|---|
| 15 | Frame length in bits, this field included |
| | Tiling: 1 bit for one tiling shared by all channels, then subframe lengths (0: 512; 10: 256; 11: 128), per step a bit per waiting channel when not shared; the last 128 samples need no bits |
| 1 (+1 +4 x 4 bits) | Stereo only: post-processing transform, skipped |
| 8 | Dynamic range gain (not applied) |
| 1 (+1 +10, +1 +10) | Trim present; start trim; end trim |
| | Subframes in time order |
| 1 | Spare |
| 1 | Another frame follows in this packet |

A subframe holds: optional fill bits, a reserved 0 bit, the stereo transform (per band (a, b) to (a - b, a + b), other
bands scaled by 181/128), a coefficients-present bit per channel, the vector-coded count, a 6-bit quantisation step
(base 90, extended in 5-bit pieces at -32 and 31) with per-channel modifiers, scale factors (first subframe: DPCM;
later: carried over and resampled to the band layout, optionally with run-level differences), and the coefficients
(vectors of four, two and one values, then run-level). A band's coefficients are scaled by
10^((step - (max scale factor - scale factor) x scale step) / 20); the inverse MDCT of n coefficients is scaled by
2 / n / 2^15. Scale factor bands follow 28 critical frequencies at the nearest of 24, 32, 44.1 or 48 kHz at or above
the stream's rate.

Each frame's output is the first 512 samples of a 768-sample buffer; the last 256 move to the front for the next
frame.

## Use

| Type | Does |
|---|---|
| `XmaStreamDecoder` | one stream, packet by packet; per packet the frames it completes; allocation-free |
| `XmaStreamDecoder.DecodeFrameAt` | the frame at a bit offset of a run of packets (headers counted, as the SHAPE XMA context's read offset), and where the next one starts; for the XMA contexts ([audio-runtime.md](audio-runtime.md)) |
| `XmaTrackDecoder` | a whole sound; drops one frame plus 64 samples at the start and 64 plus the last end trim at the end, as FFmpeg does |
| `fivestar audio export ... --decode-xma` | writes XMA2 tracks as 16-bit PCM WAVE instead of XMA2 in WAVE |

## Checked

All 1,818 XMA2 streams of `audio.rpf` were decoded and compared with FFmpeg 9.0.2's `xma2` decoder: every stream has
FFmpeg's sample count, the largest sample difference is 4.8e-7 (float rounding; at least 136 dB SNR), every frame ends
exactly where its length prefix says, and the mean step between samples at frame joins equals the mean step overall
(0.03302 against 0.03306). The decoded frame count is the AWC sample count divided by 512 for every stream. Decoding
runs about 660 times faster than real time on one core.

## Tables

`WmaProTables.cs` holds the WMA Pro code tables (scale factor, run-level, vector codes, critical frequencies) taken
from FFmpeg's `libavcodec/wmaprodata.h` (LGPL-2.1-or-later, `THIRD_PARTY_NOTICES.md`). The decoder itself is written
from the bitstream description above.

## Unknown

- The console's alignment: the game counts every frame's 512 samples out of a SHAPE context, which suggests the
  hardware outputs the first frame FFmpeg drops; the PC contexts do so. Not confirmed on hardware.
- Stereo: no XMA2 stream in the dump has two channels, so the stereo transform is untested on game data.
- Several streams interleaved by the packet skip field: `DecodePacket` ignores it, `DecodeFrameAt` follows it, and
  no packet of the dump interleaves streams.
