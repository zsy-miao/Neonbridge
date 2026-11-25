// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using Vortice.Direct3D12;

namespace FiveStar.Graphics.Pipelines;

/// <summary>
/// A D3D12 pipeline-state stream: each subobject is a u32 D3D12_PIPELINE_STATE_SUBOBJECT_TYPE followed by its payload
/// at the payload's alignment, every subobject starting pointer-aligned. Shader bytecode stays pinned until disposal.
/// </summary>
public sealed class PipelineSubobjects : IDisposable
{
    private const int PointerSize = sizeof(long);
    private const int ValueAlignment = sizeof(int);

    private readonly List<byte> _bytes = [];
    private readonly List<GCHandle> _pinned = [];

    public void AddPointer(PipelineStateSubObjectType type, IntPtr address)
    {
        Begin(type, PointerSize);
        _bytes.AddRange(BitConverter.GetBytes((long)address));
    }

    /// <summary>A D3D12_SHADER_BYTECODE subobject: pointer and length.</summary>
    public void AddShader(PipelineStateSubObjectType type, byte[] bytecode)
    {
        var handle = GCHandle.Alloc(bytecode, GCHandleType.Pinned);
        _pinned.Add(handle);
        Begin(type, PointerSize);
        _bytes.AddRange(BitConverter.GetBytes((long)handle.AddrOfPinnedObject()));
        _bytes.AddRange(BitConverter.GetBytes((long)bytecode.Length));
    }

    /// <summary>A subobject whose payload is one 4-byte-aligned value with the native layout.</summary>
    public void Add<T>(PipelineStateSubObjectType type, T value)
        where T : unmanaged
    {
        Begin(type, ValueAlignment);
        _bytes.AddRange(MemoryMarshal.AsBytes(new ReadOnlySpan<T>(ref value)));
    }

    public void AddWords(PipelineStateSubObjectType type, IReadOnlyList<int> words)
    {
        Begin(type, ValueAlignment);

        foreach (var word in words)
        {
            _bytes.AddRange(BitConverter.GetBytes(word));
        }
    }

    public unsafe ID3D12PipelineState Create(ID3D12Device device)
    {
        Align(PointerSize);
        var bytes = _bytes.ToArray();
        using var device2 = device.QueryInterface<ID3D12Device2>();

        fixed (byte* start = bytes)
        {
            var description = new PipelineStateStreamDescription { SizeInBytes = (nuint)bytes.Length, SubObjectStream = (IntPtr)start };

            return device2.CreatePipelineState<ID3D12PipelineState>(description);
        }
    }

    public void Dispose()
    {
        foreach (var handle in _pinned)
        {
            handle.Free();
        }

        _pinned.Clear();
    }

    private void Begin(PipelineStateSubObjectType type, int payloadAlignment)
    {
        Align(PointerSize);
        _bytes.AddRange(BitConverter.GetBytes((int)type));
        Align(payloadAlignment);
    }

    private void Align(int alignment)
    {
        while (_bytes.Count % alignment != 0)
        {
            _bytes.Add(0);
        }
    }
}
