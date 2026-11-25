// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// One overlapped, unbuffered file read into a sector-aligned staging buffer: the read covers the requested range
/// widened to whole sectors, and <see cref="Wait"/> hands back just the requested bytes. The buffer grows to the
/// largest read it served and is reused; nothing is allocated per read.
/// </summary>
internal sealed unsafe class ReadSlot : IDisposable
{
    /// <summary>Unbuffered reads need offsets, sizes and buffers aligned to the sector size; 4 KiB covers 512-byte and 4 KiB sectors.</summary>
    public const int Alignment = 0x1000;

    /// <summary>A first buffer that holds a 64 KiB archive chunk and its sector padding.</summary>
    private const int InitialCapacity = 0x40000;

    private const int WaitForCompletion = 1;
    private const int BitsPerByte = 8;

    private readonly FileOverlapped* _overlapped;
    private readonly nint _event;
    private byte* _buffer;
    private int _capacity;
    private nint _file;
    private int _skip;
    private int _length;
    private int _error;
    private bool _inFlight;

    public ReadSlot()
    {
        _overlapped = (FileOverlapped*)NativeMemory.AllocZeroed((nuint)sizeof(FileOverlapped));
        _event = Kernel32.CreateEventW(0, 1, 0, null);
        _buffer = (byte*)NativeMemory.AlignedAlloc(InitialCapacity, Alignment);
        _capacity = InitialCapacity;
    }

    /// <summary>True from <see cref="TryBegin"/> until <see cref="Release"/>.</summary>
    public bool Busy { get; private set; }

    /// <summary>The queue sequence number of the request this slot reads.</summary>
    public long Sequence { get; private set; }

    /// <summary>The sector-aligned span a read of <paramref name="size"/> bytes at <paramref name="offset"/> covers.</summary>
    public static ulong AlignedSpan(ulong offset, uint size)
    {
        ulong start = offset & ~(ulong)(Alignment - 1);
        ulong end = (offset + size + Alignment - 1) & ~(ulong)(Alignment - 1);

        return end - start;
    }

    /// <summary>Starts reading <paramref name="size"/> bytes at <paramref name="offset"/>; false when the aligned span exceeds <paramref name="limit"/>.</summary>
    [HotPath]
    public bool TryBegin(long sequence, nint file, ulong offset, uint size, int limit)
    {
        ulong span = AlignedSpan(offset, size);

        if (span > (ulong)limit)
        {
            return false;
        }

        EnsureCapacity((int)span);
        ulong start = offset & ~(ulong)(Alignment - 1);
        Sequence = sequence;
        Busy = true;
        _file = file;
        _skip = (int)(offset - start);

        _length = (int)size;
        _error = 0;
        *_overlapped = default;
        _overlapped->OffsetLow = (uint)start;
        _overlapped->OffsetHigh = (uint)(start >> (sizeof(uint) * BitsPerByte));
        _overlapped->Event = _event;

        Issue((uint)span);

        return true;
    }

    /// <summary>Waits for the read; S_OK with the requested bytes, or the read's HRESULT.</summary>
    [HotPath]
    public int Wait(out ReadOnlySpan<byte> data)
    {
        data = default;

        if (!_inFlight)
        {
            return HResult.FromWin32(_error);
        }

        uint transferred;
        int completed = Kernel32.GetOverlappedResult(_file, _overlapped, &transferred, WaitForCompletion);
        _inFlight = false;

        if (completed == 0)
        {
            return HResult.FromWin32(Marshal.GetLastPInvokeError());
        }

        if (transferred < (uint)(_skip + _length))
        {
            return HResult.FromWin32(Win32Constants.ErrorHandleEof);
        }

        data = new ReadOnlySpan<byte>(_buffer + _skip, _length);

        return HResult.Ok;
    }

    /// <summary>Frees the slot for the next read; the bytes from <see cref="Wait"/> stay valid until then.</summary>
    [HotPath]
    public void Release()
    {
        Busy = false;
    }

    public void Dispose()
    {
        // The kernel may still write into the buffer: let the read finish first.
        if (_inFlight)
        {
            Wait(out _);
        }

        NativeMemory.AlignedFree(_buffer);
        NativeMemory.Free(_overlapped);
        _ = Kernel32.CloseHandle(_event);
        _buffer = null;
    }

    [HotPath]
    private void Issue(uint span)
    {
        if (Kernel32.ReadFile(_file, _buffer, span, null, _overlapped) != 0)
        {
            _inFlight = true;

            return;
        }

        int error = Marshal.GetLastPInvokeError();

        if (error == Win32Constants.ErrorIoPending)
        {
            _inFlight = true;

            return;
        }

        _inFlight = false;
        _error = error;
    }

    private void EnsureCapacity(int needed)
    {
        if (needed <= _capacity)
        {
            return;
        }

        NativeMemory.AlignedFree(_buffer);
        _buffer = (byte*)NativeMemory.AlignedAlloc((nuint)needed, Alignment);
        _capacity = needed;
    }
}
