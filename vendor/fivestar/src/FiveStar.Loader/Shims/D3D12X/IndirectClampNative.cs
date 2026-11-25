// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>d3dcompiler_47.dll's D3DCompile (d3dcompiler.h), for the shim's own small programs; part of Windows.</summary>
internal static unsafe class IndirectClampNative
{
    /// <summary>ID3DBlob::GetBufferPointer and GetBufferSize (d3dcommon.h).</summary>
    public const int BlobPointerSlot = 3;
    public const int BlobSizeSlot = 4;

    [DllImport("d3dcompiler_47")]
    public static extern int D3DCompile(byte* source, nuint size, byte* name, nint defines, nint include, byte* entry, byte* target, uint flags1, uint flags2, nint* code, nint* errors);
}
