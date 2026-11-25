// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Bvh;

/// <summary>
/// <c>--raytracing-gpu</c>, inventory only (docs/bvh-builder.md): the shipped constructor of XboxBvhBuilder runs on our
/// device, and every compute pipeline it asks for through device slot 62 while it runs is noted (its CS container's
/// address and size, its root signature) and answered with a stand-in that is never built, so the constructor completes
/// and no builder program runs. The builder's entry points stay ours.
/// </summary>
internal static unsafe class BvhBuilderInventory
{
    /// <summary>D3D12_COMPUTE_PIPELINE_STATE_DESC: root signature, then the CS bytecode pointer and length.</summary>
    private const int RootSignatureField = 0;
    private const int BytecodeField = 8;
    private const int LengthField = 16;

    [ThreadStatic]
    private static List<(nint Bytecode, nint Length, nint RootSignature)>? t_pipelines;

    /// <summary>The pipelines the constructor asked for while <paramref name="construct"/> ran on this thread.</summary>
    public static List<(nint Bytecode, nint Length, nint RootSignature)> During(Action construct)
    {
        var pipelines = new List<(nint, nint, nint)>();
        t_pipelines = pipelines;

        try
        {
            construct();
        }
        finally
        {
            t_pipelines = null;
        }

        return pipelines;
    }

    /// <summary>
    /// Device slot 62 while the constructor runs on this thread: notes the desc and gives a stand-in that is never
    /// built; false otherwise (the slot runs as always).
    /// </summary>
    public static bool Capture(byte* desc, nint* result)
    {
        var pipelines = t_pipelines;

        if (pipelines is null || desc == null || result == null)
        {
            return false;
        }

        pipelines.Add((*(nint*)(desc + BytecodeField), *(nint*)(desc + LengthField), *(nint*)(desc + RootSignatureField)));
        *result = ShimContext.Runtime.D3D12X.Pipelines?.StandIns.Unbuilt() ?? 0;

        return true;
    }
}
