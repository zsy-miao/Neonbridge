// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Upload memory for the user data of draws and dispatches (root parameter 0 of every translated program, a root CBV):
/// one ring that every list takes 512-byte blocks from. A block is reused after <see cref="Blocks"/> later blocks; the
/// GPU must have finished the draw by then (about 16 frames of 8,000 draws).
/// </summary>
internal sealed unsafe class UserDataRing
{
    /// <summary>128 user-data dwords, a multiple of the 256-byte CBV alignment.</summary>
    public const uint BlockBytes = CommandListBindings.UserDataDwords * sizeof(uint);

    public const uint Blocks = 131072;

    /// <summary>D3D12_RESOURCE_STATE_GENERIC_READ, the state of upload buffers.</summary>
    private const int GenericRead = 0xAC3;

    private readonly byte* _data;
    private readonly ulong _gpu;
    private long _next;

    private UserDataRing(byte* data, ulong gpu)
    {
        _data = data;
        _gpu = gpu;
    }

    /// <summary>The ring on <paramref name="device"/>; null when the upload buffer cannot be made.</summary>
    public static UserDataRing? Create(nint device)
    {
        var properties = new HeapProperties { Type = HeapProperties.Upload };
        nint buffer = DesktopCalls.CreateBuffer(device, properties, (ulong)Blocks * BlockBytes, 0, GenericRead);
        byte* data = buffer == 0 ? null : DesktopCalls.Map(buffer);

        if (data == null)
        {
            return null;
        }

        return new UserDataRing(data, DesktopCalls.GpuAddress(buffer));
    }

    /// <summary>
    /// Copies the first <paramref name="dwords"/> of one block of user data and returns the block's GPU address. A
    /// root signature uses few of the 128 dwords (its base SGPR plus its parameters'), and the copy goes to
    /// write-combined memory: copying 32 dwords instead of 128 took an upload from 115 ns to 95 ns (D3D12XUploadBenchmarks).
    /// </summary>
    [HotPath]
    public ulong Upload(uint* userData, uint dwords)
    {
        ulong block = (ulong)(Interlocked.Increment(ref _next) - 1) % Blocks;
        ulong offset = block * BlockBytes;
        uint bytes = BlockBytes;

        if (dwords < CommandListBindings.UserDataDwords)
        {
            bytes = dwords * sizeof(uint);
        }

        Buffer.MemoryCopy(userData, _data + offset, BlockBytes, bytes);

        return _gpu + offset;
    }
}
