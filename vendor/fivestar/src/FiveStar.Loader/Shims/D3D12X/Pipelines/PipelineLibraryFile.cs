// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.IO.MemoryMappedFiles;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// One segment file of the pipeline cache (docs/d3d12x-pipelines.md, Pipeline cache): a header, the 16-byte names of
/// the pipelines it holds, then a serialized ID3D12PipelineLibrary at <see cref="BlobAlignment"/>. A file is written
/// beside its final name and renamed into place, so readers only see whole files.
/// </summary>
internal static unsafe class PipelineLibraryFile
{
    public const string Extension = ".psolib";
    public const string PartialExtension = ".tmp";

    /// <summary>The bytes of one pipeline name (<see cref="DesktopStream.Key"/> as binary).</summary>
    public const int KeySize = 16;

    // Header: "FSPL" @0, u32 version @4, 32-byte salt @8, u32 name count @0x28, u32 0 @0x2C, u64 blob size @0x30.
    private const uint Magic = 0x4C505346;
    private const uint Version = 1;
    private const int VersionField = 0x04;
    private const int SaltField = 0x08;
    private const int SaltSize = 32;
    private const int CountField = 0x28;
    private const int BlobSizeField = 0x30;
    private const int HeaderSize = 0x38;
    private const int BlobAlignment = 256;

    /// <summary>
    /// Writes <paramref name="keys"/> and <paramref name="library"/> serialized to <paramref name="path"/>; returns the
    /// file size, or the failing HRESULT of Serialize as a negative number.
    /// </summary>
    public static long Write(string path, ReadOnlySpan<byte> salt, IReadOnlyList<string> keys, nint library)
    {
        var size = (long)DesktopPipelineLibrary.SerializedSize(library);
        var offset = BlobOffset(keys.Count);
        var partial = path + PartialExtension;
        int hr;

        using (var file = new FileStream(partial, FileMode.Create, FileAccess.ReadWrite, FileShare.None))
        {
            file.SetLength(offset + size);

            using var map = MemoryMappedFile.CreateFromFile(file, null, offset + size, MemoryMappedFileAccess.ReadWrite, HandleInheritability.None, leaveOpen: true);
            using var view = map.CreateViewAccessor(0, offset + size, MemoryMappedFileAccess.ReadWrite);
            var start = Acquire(view);

            try
            {
                var bytes = new Span<byte>(start, (int)offset);
                WriteHeader(bytes, salt, keys, size);
                hr = DesktopPipelineLibrary.Serialize(library, start + offset, (nuint)size);
                view.Flush();
            }
            finally
            {
                view.SafeMemoryMappedViewHandle.ReleasePointer();
            }
        }

        if (HResult.Failed(hr))
        {
            File.Delete(partial);

            return hr;
        }

        File.Move(partial, path, overwrite: true);

        return offset + size;
    }

    /// <summary>The file mapped for reading, or null when it is not a segment made with <paramref name="salt"/>.</summary>
    public static PipelineLibraryView? Open(string path, ReadOnlySpan<byte> salt)
    {
        var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read | FileShare.Delete);

        if (file.Length < HeaderSize)
        {
            file.Dispose();

            return null;
        }

        var map = MemoryMappedFile.CreateFromFile(file, null, 0, MemoryMappedFileAccess.Read, HandleInheritability.None, leaveOpen: false);
        var view = map.CreateViewAccessor(0, 0, MemoryMappedFileAccess.Read);
        var start = Acquire(view);

        var header = new ReadOnlySpan<byte>(start, HeaderSize);
        var count = BinaryPrimitives.ReadInt32LittleEndian(header[CountField..]);
        var size = BinaryPrimitives.ReadInt64LittleEndian(header[BlobSizeField..]);
        var valid = BinaryPrimitives.ReadUInt32LittleEndian(header) == Magic
            && BinaryPrimitives.ReadUInt32LittleEndian(header[VersionField..]) == Version
            && header.Slice(SaltField, SaltSize).SequenceEqual(salt)
            && count >= 0 && BlobOffset(count) + size <= file.Length;

        if (!valid)
        {
            view.SafeMemoryMappedViewHandle.ReleasePointer();
            view.Dispose();
            map.Dispose();

            return null;
        }

        return new PipelineLibraryView(map, view, Keys(start, count), (nint)(start + BlobOffset(count)), (nuint)size);
    }

    private static void WriteHeader(Span<byte> bytes, ReadOnlySpan<byte> salt, IReadOnlyList<string> keys, long size)
    {
        bytes.Clear();
        BinaryPrimitives.WriteUInt32LittleEndian(bytes, Magic);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes[VersionField..], Version);
        salt.CopyTo(bytes[SaltField..]);
        BinaryPrimitives.WriteInt32LittleEndian(bytes[CountField..], keys.Count);
        BinaryPrimitives.WriteInt64LittleEndian(bytes[BlobSizeField..], size);

        for (var index = 0; index < keys.Count; index++)
        {
            Convert.FromHexString(keys[index]).CopyTo(bytes[(HeaderSize + (index * KeySize))..]);
        }
    }

    private static List<string> Keys(byte* start, int count)
    {
        var keys = new List<string>(count);

        for (var index = 0; index < count; index++)
        {
            keys.Add(Convert.ToHexStringLower(new ReadOnlySpan<byte>(start + HeaderSize + (index * KeySize), KeySize)));
        }

        return keys;
    }

    private static long BlobOffset(int count)
    {
        long end = HeaderSize + ((long)count * KeySize);

        return (end + BlobAlignment - 1) / BlobAlignment * BlobAlignment;
    }

    private static byte* Acquire(MemoryMappedViewAccessor view)
    {
        byte* start = null;
        view.SafeMemoryMappedViewHandle.AcquirePointer(ref start);

        return start + view.PointerOffset;
    }
}
