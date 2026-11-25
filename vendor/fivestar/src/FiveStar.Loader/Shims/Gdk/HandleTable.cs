// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// Opaque native handles for managed objects. Each handle is a distinct 16-byte native block (an 8-character tag and
/// an id), so handles are real, unique pointers. Closed handles are not freed: a late call with a stale handle fails
/// the lookup instead of reaching reused memory.
/// </summary>
internal sealed unsafe class HandleTable<T>
    where T : class
{
    private const int TagLength = sizeof(ulong);
    private const int BlockSize = 2 * sizeof(ulong);

    private readonly ConcurrentDictionary<nint, T> _objects = new();
    private readonly ulong _tag;
    private long _next;

    public HandleTable(string tag)
    {
        var bytes = new byte[TagLength];
        Encoding.ASCII.GetBytes(tag, bytes);
        _tag = BinaryPrimitives.ReadUInt64LittleEndian(bytes);
    }

    public int Count => _objects.Count;

    public nint Add(T value)
    {
        var block = (ulong*)NativeMemory.AllocZeroed(BlockSize);
        block[0] = _tag;
        block[1] = (ulong)Interlocked.Increment(ref _next);
        _objects[(nint)block] = value;

        return (nint)block;
    }

    [HotPath]
    public T? Get(nint handle)
    {
        if (handle == 0)
        {
            return null;
        }

        return _objects.GetValueOrDefault(handle);
    }

    public bool Remove(nint handle)
    {
        return _objects.TryRemove(handle, out _);
    }
}
