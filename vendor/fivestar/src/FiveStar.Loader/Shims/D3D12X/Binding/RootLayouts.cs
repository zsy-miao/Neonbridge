// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The <see cref="RootLayout"/> of every root signature the game created (device slot 16), by desktop object. A layout
/// stays allocated for the process: lists may hold it after the root signature is gone, and a reused address gets a new
/// one. Each counts by the user SGPRs its parameters take: a compute program has s4 to s15 for them and a graphics
/// stage s8 to s31; past that the console would pass the rest through a packed array (Leonida's notes), which no program
/// of the game reads (no compute program reads s0 to s3; docs/pipeline-cache.md, Bindings).
/// </summary>
internal sealed unsafe class RootLayouts
{
    /// <summary>User SGPRs a compute program (s4 to s15) and a graphics stage (s8 to s31) have for root parameters.</summary>
    public const int ComputeSgprs = 12;
    public const int GraphicsSgprs = 24;

    private readonly ConcurrentDictionary<nint, nint> _layouts = new();
    private long _registered;
    private long _overComputeSgprs;
    private long _overGraphicsSgprs;
    private int _mostSgprs;

    /// <summary>Root signatures registered, those whose parameters take more SGPRs than a compute program and than a graphics stage has, and the most any takes.</summary>
    public string Summary => $"{Interlocked.Read(ref _registered)} root signatures, {Interlocked.Read(ref _overComputeSgprs)} over the {ComputeSgprs} compute user SGPRs, {Interlocked.Read(ref _overGraphicsSgprs)} over the {GraphicsSgprs} graphics ones, at most {Volatile.Read(ref _mostSgprs)} SGPRs";

    /// <summary>Registers the layout of <paramref name="blob"/> for <paramref name="rootSignature"/>; false when the blob is not one.</summary>
    public bool Register(nint rootSignature, ReadOnlySpan<byte> blob)
    {
        if (rootSignature == 0 || !RootSignatureParser.TryParse(blob, out var layout))
        {
            return false;
        }

        Count(layout.Sgprs());
        var native = (RootLayout*)NativeMemory.Alloc((nuint)sizeof(RootLayout));
        *native = layout;
        _layouts[rootSignature] = (nint)native;

        return true;
    }

    private void Count(int sgprs)
    {
        Interlocked.Increment(ref _registered);

        if (sgprs > ComputeSgprs)
        {
            Interlocked.Increment(ref _overComputeSgprs);
        }

        if (sgprs > GraphicsSgprs)
        {
            Interlocked.Increment(ref _overGraphicsSgprs);
        }

        int most = Volatile.Read(ref _mostSgprs);

        while (sgprs > most && Interlocked.CompareExchange(ref _mostSgprs, sgprs, most) != most)
        {
            most = Volatile.Read(ref _mostSgprs);
        }
    }

    /// <summary>The layout of a registered root signature; null for any other pointer.</summary>
    [HotPath]
    public RootLayout* Find(nint rootSignature)
    {
        if (rootSignature != 0 && _layouts.TryGetValue(rootSignature, out nint layout))
        {
            return (RootLayout*)layout;
        }

        return null;
    }
}
