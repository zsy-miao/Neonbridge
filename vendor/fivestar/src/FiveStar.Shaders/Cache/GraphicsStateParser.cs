// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Reads the fixed-function state entry of a graphics pipeline (380 to 520 bytes). It is a fixed run of context
/// register values; the mesh layout holds 0x54 more bytes of NGG registers before the pixel-shader block, so every
/// field from SPI_PS_INPUT_ENA (0x58) on moves by that much. A variable tail follows the fixed part.
/// </summary>
public static class GraphicsStateParser
{
    /// <summary>Number of colour targets (CB_BLEND0..7_CONTROL).</summary>
    public const int Targets = 8;

    private const uint VersionPlain = 0x4005D800;
    private const uint VersionB = 0x4005D808;
    private const uint VersionC = 0x4005D810;
    private const int KindField = 0x04;
    private const uint VertexKind = 4;
    private const uint MeshKind = 6;
    private const int MeshShift = 0x54;
    private const int VertexSize = 380;
    private const int MeshSize = 464;

    // Offsets in the vertex layout. 0x2C is ahead of the moved block.
    private const int GsOutPrimTypeField = 0x2C;
    private const int ColFormatField = 0x70;
    private const int ShaderMaskField = 0x78;
    private const int ClipControlField = 0x9C;
    private const int ModeControlField = 0xA0;
    private const int PolyOffsetClampField = 0xAC;
    private const int PolyOffsetScaleField = 0xB0;
    private const int PolyOffsetOffsetField = 0xB4;
    private const int DepthControlField = 0xB8;
    private const int StencilControlField = 0xBC;
    private const int StencilRefMaskField = 0xC0;
    private const int StencilRefMaskBackField = 0xC4;
    private const int TargetMaskField = 0xCC;
    private const int BlendControlField = 0xD0;
    private const int DownconvertField = 0xF4;
    private const int BlendOptEpsilonField = 0xF8;
    private const int BlendOptControlField = 0xFC;
    private const int EqaaField = 0x120;

    /// <summary>The registers of a state entry, or null when its version or layout is not one of the known ones.</summary>
    public static GraphicsStateRegisters? Parse(ReadOnlySpan<byte> state)
    {
        if (state.Length < VertexSize)
        {
            return null;
        }

        var version = U32(state, 0);
        var kind = U32(state, KindField);
        var known = version == VersionPlain || version == VersionB || version == VersionC;

        if (!known || (kind != VertexKind && kind != MeshKind))
        {
            return null;
        }

        var mesh = kind == MeshKind;

        if (mesh && state.Length < MeshSize)
        {
            return null;
        }

        return Read(state, version, mesh);
    }

    private static GraphicsStateRegisters Read(ReadOnlySpan<byte> state, uint version, bool mesh)
    {
        var shift = 0;

        if (mesh)
        {
            shift = MeshShift;
        }

        var blend = new uint[Targets];

        for (var target = 0; target < Targets; target++)
        {
            blend[target] = U32(state, BlendControlField + shift + (target * sizeof(uint)));
        }

        return new GraphicsStateRegisters(
            version,
            mesh,
            U32(state, GsOutPrimTypeField),
            U32(state, ColFormatField + shift),
            U32(state, ShaderMaskField + shift),
            U32(state, ClipControlField + shift),
            U32(state, ModeControlField + shift),
            F32(state, PolyOffsetClampField + shift),
            F32(state, PolyOffsetScaleField + shift),
            F32(state, PolyOffsetOffsetField + shift),
            U32(state, DepthControlField + shift),
            U32(state, StencilControlField + shift),
            U32(state, StencilRefMaskField + shift),
            U32(state, StencilRefMaskBackField + shift),
            U32(state, TargetMaskField + shift),
            blend,
            U32(state, DownconvertField + shift),
            U32(state, BlendOptEpsilonField + shift),
            U32(state, BlendOptControlField + shift),
            U32(state, EqaaField + shift));
    }

    private static uint U32(ReadOnlySpan<byte> state, int field)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(state[field..]);
    }

    private static float F32(ReadOnlySpan<byte> state, int field)
    {
        return BinaryPrimitives.ReadSingleLittleEndian(state[field..]);
    }
}
