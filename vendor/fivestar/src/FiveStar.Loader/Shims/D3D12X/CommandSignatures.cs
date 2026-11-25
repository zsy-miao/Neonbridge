// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Globalization;
using System.Text;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The game's command-signature descriptions by wrapper, as text and stride, for the breadcrumb report
/// (docs/d3d12x-shim.md#queue-progress). Each new one is logged once as a detail.
/// </summary>
internal static unsafe class CommandSignatures
{
    private static readonly ConcurrentDictionary<nint, (uint Stride, string Text)> Known = new();
    private static readonly ConcurrentDictionary<nint, (int Kind, uint Stride)> Clampables = new();

    /// <summary>Notes the game's <paramref name="description"/> for the created <paramref name="signature"/> wrapper.</summary>
    public static void Created(nint signature, CommandSignatureDescription* description, nint rootSignature)
    {
        string root = "set";

        if (rootSignature == 0)
        {
            root = "none";
        }

        var text = new StringBuilder();
        text.Append(CultureInfo.InvariantCulture, $"stride {description->ByteStride}, root signature {root}:");

        for (uint i = 0; i < description->ArgumentCount; i++)
        {
            var argument = description->Arguments[i];
            text.Append(CultureInfo.InvariantCulture, $" [type 0x{argument.Type:x} {argument.First} {argument.Second} {argument.Third}]");
        }

        string described = text.ToString();
        Known[signature] = (description->ByteStride, described);
        NoteClampable(signature, description);
        ShimContext.Runtime.Log.Detail($"  [d3d12_xs] command signature 0x{signature:x}: {described}");
    }

    /// <summary>
    /// The argument kind (<see cref="IndirectClamp"/> numbering) and stride of a signature whose only argument is a draw,
    /// dispatch or mesh dispatch at offset 0; kind -1 for any other.
    /// </summary>
    public static (int Kind, uint Stride) Clampable(nint signature)
    {
        if (Clampables.TryGetValue(signature, out var clampable))
        {
            return clampable;
        }

        return (-1, 0);
    }

    private static void NoteClampable(nint signature, CommandSignatureDescription* description)
    {
        if (description->ArgumentCount != 1 || description->ByteStride % sizeof(uint) != 0)
        {
            return;
        }

        int kind = description->Arguments[0].Type;
        uint smallest;

        switch (kind)
        {
            case IndirectClamp.DrawKind:
                smallest = DrawBytes;
                break;
            case IndirectClamp.DrawIndexedKind:
                smallest = DrawIndexedBytes;
                break;
            case IndirectClamp.DispatchKind or IndirectClamp.MeshKind or IndirectArgument.XboxDispatchType:
                smallest = DispatchBytes;
                break;
            default:
                return;
        }

        if (kind == IndirectArgument.XboxDispatchType)
        {
            kind = IndirectClamp.DispatchKind;
        }

        if (description->ByteStride >= smallest)
        {
            Clampables[signature] = (kind, description->ByteStride);
        }
    }

    /// <summary>D3D12_DRAW_ARGUMENTS, D3D12_DRAW_INDEXED_ARGUMENTS and D3D12_DISPATCH_ARGUMENTS bytes.</summary>
    private const uint DrawBytes = 16;
    private const uint DrawIndexedBytes = 20;
    private const uint DispatchBytes = 12;

    public static string Describe(nint signature)
    {
        if (Known.TryGetValue(signature, out var known))
        {
            return known.Text;
        }

        return string.Create(CultureInfo.InvariantCulture, $"0x{signature:x}");
    }

    public static uint Stride(nint signature)
    {
        return Known.GetValueOrDefault(signature).Stride;
    }
}
