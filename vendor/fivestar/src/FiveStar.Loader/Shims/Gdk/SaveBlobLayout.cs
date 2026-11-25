// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Text;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// Lays blobs out in the caller's buffer: XGameSaveBlob[count], then the data, then the names, packed. A caller that
/// sized its buffer for the blobs and their data only gets names the loader keeps (one copy per distinct name), so a
/// read never fails on how the caller counted the names.
/// </summary>
internal static unsafe class SaveBlobLayout
{
    private static readonly ConcurrentDictionary<string, nint> KeptNames = new(StringComparer.Ordinal);

    /// <summary>The full size: blobs, data and names.</summary>
    public static nuint Size(IReadOnlyList<SaveBlobData> blobs)
    {
        return MinimumSize(blobs) + (nuint)blobs.Sum(b => Encoding.UTF8.GetByteCount(b.Name) + 1);
    }

    /// <summary>The smallest buffer a read fills: the blobs and their data.</summary>
    public static nuint MinimumSize(IReadOnlyList<SaveBlobData> blobs)
    {
        return (nuint)((long)blobs.Count * SaveLayout.BlobSize + blobs.Sum(b => (long)b.Data!.Length));
    }

    public static void Write(IReadOnlyList<SaveBlobData> blobs, byte* buffer, nuint bufferSize)
    {
        byte* tail = buffer + (blobs.Count * SaveLayout.BlobSize);

        for (int i = 0; i < blobs.Count; i++)
        {
            byte* entry = buffer + (i * SaveLayout.BlobSize);
            var data = blobs[i].Data!;
            data.CopyTo(new Span<byte>(tail, data.Length));
            *(ulong*)(entry + SaveLayout.BlobSizeOffset) = (uint)data.Length;
            *(nint*)(entry + SaveLayout.BlobDataOffset) = (nint)tail;
            tail += data.Length;
        }

        bool namesFit = bufferSize >= Size(blobs);

        for (int i = 0; i < blobs.Count; i++)
        {
            byte* entry = buffer + (i * SaveLayout.BlobSize);

            if (namesFit)
            {
                var name = Encoding.UTF8.GetBytes(blobs[i].Name);
                name.CopyTo(new Span<byte>(tail, name.Length));
                tail[name.Length] = 0;
                *(nint*)(entry + SaveLayout.BlobNameOffset) = (nint)tail;
                tail += name.Length + 1;
            }
            else
            {
                *(nint*)(entry + SaveLayout.BlobNameOffset) = KeptNames.GetOrAdd(blobs[i].Name, n => (nint)Utf8Text.Allocate(n));
            }
        }
    }
}
