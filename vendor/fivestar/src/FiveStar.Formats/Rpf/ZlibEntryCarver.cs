// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers;
using FiveStar.Formats.Common;
using FiveStar.Formats.Pso;

namespace FiveStar.Formats.Rpf;

/// <summary>
/// Recovers the entries of an RPF9 archive without its (encrypted) table of contents. An entry is a run of 64 KiB
/// chunks, each at the 16-byte boundary after the previous one, each a zlib stream or stored raw; every chunk but the
/// last inflates to exactly 64 KiB. A raw chunk is taken when its size is known: for PSO files from the <c>CHKS</c>
/// total, otherwise only as a full 64 KiB chunk followed by a zlib chunk. A raw tail of another kind is left out.
/// </summary>
public sealed class ZlibEntryCarver
{
    private readonly IByteSource _source;
    private readonly ZlibStreamProbe _probe;

    public ZlibEntryCarver(IByteSource source, ZlibStreamProbe probe)
    {
        _source = source;
        _probe = probe;
    }

    /// <summary>The entries whose first chunk starts before <paramref name="limit"/> (0 = whole archive), in archive order.</summary>
    public IEnumerable<CarvedBlob> Carve(long limit)
    {
        return CarveFrom(0, limit);
    }

    /// <summary>
    /// The entries whose first chunk starts at or after <paramref name="from"/> and before <paramref name="limit"/>
    /// (0 = the end of the archive). <paramref name="from"/> should be an entry's first chunk.
    /// </summary>
    public IEnumerable<CarvedBlob> CarveFrom(long from, long limit)
    {
        var end = _source.Length;

        if (limit > 0)
        {
            end = Math.Min(end, limit);
        }

        var position = from;
        long previousEnd = -1;
        var previousKind = BlobKind.Bin;

        while (position < end)
        {
            var start = _probe.NextHeader(position, end);

            if (start < 0)
            {
                yield break;
            }

            if (!_probe.TryInflate(start, ZlibStreamProbe.MaxStreamOutput, out var used))
            {
                position = start + 1;

                continue;
            }

            var entry = ReadEntry(start, used);
            var kind = BlobClassifier.Classify(entry.Data);

            if (start == previousEnd && IsVirtual(previousKind) && !IsVirtual(kind) && kind != BlobKind.Psin)
            {
                kind = BlobKind.Phys;
            }

            yield return new CarvedBlob(start, kind, entry.Data, entry.RawChunks);

            previousEnd = entry.Next;
            previousKind = kind;
            position = entry.Next;
        }
    }

    private static bool IsVirtual(BlobKind kind)
    {
        return kind == BlobKind.Rsc || kind == BlobKind.Prd0;
    }

    private Entry ReadEntry(long start, int used)
    {
        var blob = new ArrayBufferWriter<byte>(_probe.Output.Length);
        blob.Write(_probe.Output);
        var state = new ChunkRun(ZlibStreamProbe.Align(start + used), blob.WrittenSpan.StartsWith(PsoFraming.FileTag));

        while (blob.WrittenCount % ZlibStreamProbe.ChunkSize == 0)
        {
            if (!AppendChunk(blob, state))
            {
                break;
            }
        }

        if (!state.IsPso)
        {
            return new Entry(blob.WrittenSpan.ToArray(), state.Position, state.RawChunks);
        }

        return TrimPso(blob, state);
    }

    // One more chunk of the run; false when the entry ends here.
    private bool AppendChunk(ArrayBufferWriter<byte> blob, ChunkRun state)
    {
        long? total = null;

        if (state.IsPso)
        {
            total = PsoFraming.DeclaredSize(blob.WrittenSpan);

            if (total.HasValue && total.Value <= blob.WrittenCount)
            {
                return false;
            }
        }

        if (_probe.TryChunk(state.Position, out var used))
        {
            blob.Write(_probe.Output);
            state.Position = ZlibStreamProbe.Align(state.Position + used);
            state.RawAt = null;

            return true;
        }

        return AppendRawChunk(blob, state, total);
    }

    private bool AppendRawChunk(ArrayBufferWriter<byte> blob, ChunkRun state, long? total)
    {
        long want = ZlibStreamProbe.ChunkSize;

        if (state.IsPso)
        {
            if (total.HasValue && total.Value != 0)
            {
                want = Math.Min(ZlibStreamProbe.ChunkSize, total.Value - blob.WrittenCount);
            }
        }
        else if (!_probe.TryChunk(state.Position + ZlibStreamProbe.ChunkSize, out _))
        {
            return false;
        }

        want = Math.Min(want, _source.Length - state.Position);

        if (want <= 0)
        {
            return false;
        }

        state.RawAt = (state.Position, blob.WrittenCount);
        blob.Write(_source.Read(state.Position, (int)want));
        state.Position = ZlibStreamProbe.Align(state.Position + want);
        state.RawChunks++;

        return true;
    }

    // A PSO entry ends at its declared size; when a raw chunk read past it, the next scan starts after the real end.
    private static Entry TrimPso(ArrayBufferWriter<byte> blob, ChunkRun state)
    {
        var total = PsoFraming.DeclaredSize(blob.WrittenSpan);

        if (!total.HasValue || total.Value == 0 || total.Value >= blob.WrittenCount)
        {
            return new Entry(blob.WrittenSpan.ToArray(), state.Position, state.RawChunks);
        }

        var next = state.Position;

        if (state.RawAt.HasValue)
        {
            var (rawOffset, lengthBefore) = state.RawAt.Value;
            next = ZlibStreamProbe.Align(rawOffset + total.Value - lengthBefore);
        }

        return new Entry(blob.WrittenSpan[..(int)total.Value].ToArray(), next, state.RawChunks);
    }

    private sealed record Entry(byte[] Data, long Next, int RawChunks);

    private sealed class ChunkRun
    {
        public ChunkRun(long position, bool isPso)
        {
            Position = position;
            IsPso = isPso;
        }

        public long Position { get; set; }

        public bool IsPso { get; }

        public (long Offset, int LengthBefore)? RawAt { get; set; }

        public int RawChunks { get; set; }
    }
}
