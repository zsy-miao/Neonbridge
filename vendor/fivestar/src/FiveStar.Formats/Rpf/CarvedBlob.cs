// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Rpf;

/// <summary>One entry recovered from an archive: where its first chunk starts, its kind and its inflated bytes.</summary>
/// <param name="Offset">Archive offset of the entry's first chunk.</param>
/// <param name="Kind">The kind judged from the leading bytes (or <see cref="BlobKind.Phys"/> after a virtual segment).</param>
/// <param name="Data">The entry's bytes, all chunks joined.</param>
/// <param name="RawChunks">How many of its chunks were stored raw instead of zlib.</param>
public sealed record CarvedBlob(long Offset, BlobKind Kind, byte[] Data, int RawChunks);
