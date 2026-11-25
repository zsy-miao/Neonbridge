// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Root arguments to user SGPRs. The console packs root parameters into user SGPRs from s8 (vertex, pixel and mesh
/// programs) or s4 (compute) (docs/pipeline-cache.md, Bindings); a translated program reads SGPR n as
/// <c>user_sgpr(n)</c>, dword n of <c>cbuffer UserData</c>, except mesh and geometry programs, which read
/// <c>user_sgpr(n - 8)</c>. One CBV serves both stages of a pipeline, so graphics SGPRs 8 to 15 are also written to
/// dwords 0 to 7, which no program reads as its own (s0 to s7 never hold a root parameter).
/// </summary>
internal static unsafe class UserData
{
    public const uint GraphicsBase = 8;
    public const uint ComputeBase = 4;

    /// <summary>System SGPRs before the user data of merged (mesh, geometry) programs.</summary>
    private const uint MergedShift = 8;

    /// <summary>Dwords per 16 bytes, the unit of the upload.</summary>
    private const uint DwordsPerVector = 4;

    /// <summary>Bits of the low dword of an address.</summary>
    private const int HighShift = 32;

    /// <summary>Writes dword <paramref name="dword"/> of root parameter <paramref name="parameter"/>; ignored when the parameter has no such dword.</summary>
    [HotPath]
    public static void Write(CommandListBindings* bindings, bool compute, uint parameter, uint dword, uint value)
    {
        RootLayout* layout = compute ? bindings->ComputeLayout : bindings->GraphicsLayout;

        if (layout == null || parameter >= (uint)layout->Count || dword >= layout->Sizes[parameter])
        {
            return;
        }

        if (compute)
        {
            WriteSgpr(bindings->Compute, ComputeBase + layout->Offsets[parameter] + dword, value);
            bindings->ComputeDirty = 1;

            return;
        }

        uint sgpr = GraphicsBase + layout->Offsets[parameter] + dword;
        WriteSgpr(bindings->Graphics, sgpr, value);

        if (sgpr < GraphicsBase + MergedShift)
        {
            WriteSgpr(bindings->Graphics, sgpr - MergedShift, value);
        }

        bindings->GraphicsDirty = 1;
    }

    /// <summary>
    /// The user-data dwords a draw or dispatch must upload: up to the last SGPR of the bind point's root signature (the
    /// graphics base covers dwords 0 to 7, the copy mesh programs read), rounded up to 16 bytes; all 128 without a
    /// known layout.
    /// </summary>
    [HotPath]
    public static uint Used(CommandListBindings* bindings, bool compute)
    {
        RootLayout* layout = compute ? bindings->ComputeLayout : bindings->GraphicsLayout;

        if (layout == null)
        {
            return CommandListBindings.UserDataDwords;
        }

        uint used = GraphicsBase;

        if (compute)
        {
            used = ComputeBase;
        }

        used = (used + (uint)layout->Sgprs() + DwordsPerVector - 1) & ~(DwordsPerVector - 1);

        if (used > CommandListBindings.UserDataDwords)
        {
            used = CommandListBindings.UserDataDwords;
        }

        return used;
    }

    /// <summary>Writes a 64-bit address to the two SGPRs of a root descriptor.</summary>
    [HotPath]
    public static void WriteAddress(CommandListBindings* bindings, bool compute, uint parameter, ulong address)
    {
        Write(bindings, compute, parameter, 0, (uint)address);
        Write(bindings, compute, parameter, 1, (uint)(address >> HighShift));
    }

    [HotPath]
    private static void WriteSgpr(uint* data, uint sgpr, uint value)
    {
        if (sgpr < CommandListBindings.UserDataDwords)
        {
            data[sgpr] = value;
        }
    }
}
