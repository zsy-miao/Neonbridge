// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Turns the state registers of a graphics pipeline into D3D12 fixed-function state. Bitfields and hardware enums:
/// AMD RDNA2 register reference (gc_10_3_0_sh_mask.h, gfx9+ merged enums: BlendOp, CombFunc, CompareFrag, StencilOp,
/// VGT_GS_OUTPRIM_TYPE). D3D12 enum values: d3d12.h.
/// </summary>
public static class FixedFunctionDecoder
{
    private const int NibbleBits = 4;
    private const uint Nibble = 0xF;
    private const uint ThreeBits = 0x7;
    private const uint FiveBits = 0x1F;
    private const uint ByteMask = 0xFF;

    // CB_BLENDn_CONTROL.
    private const int ColorSrcShift = 0;
    private const int ColorCombShift = 5;
    private const int ColorDestShift = 8;
    private const int AlphaSrcShift = 16;
    private const int AlphaCombShift = 21;
    private const int AlphaDestShift = 24;
    private const int SeparateAlphaBit = 29;
    private const int EnableBit = 30;

    // DB_DEPTH_CONTROL, DB_STENCIL_CONTROL, DB_STENCILREFMASK.
    private const int StencilEnableBit = 0;
    private const int ZEnableBit = 1;
    private const int ZWriteBit = 2;
    private const int DepthBoundsBit = 3;
    private const int ZFuncShift = 4;
    private const int BackfaceBit = 7;
    private const int StencilFuncShift = 8;
    private const int StencilFuncBackShift = 20;
    private const int StencilFailShift = 0;
    private const int StencilPassShift = 4;
    private const int StencilDepthFailShift = 8;
    private const int StencilBackShift = 12;
    private const int StencilMaskShift = 8;
    private const int StencilWriteMaskShift = 16;

    // PA_SU_SC_MODE_CNTL, PA_CL_CLIP_CNTL, DB_EQAA.
    private const int CullFrontBit = 0;
    private const int CullBackBit = 1;
    private const int FaceBit = 2;
    private const int PolyModeShift = 3;
    private const uint PolyModeMask = 0x3;
    private const int FrontPtypeShift = 5;
    private const uint LinesPtype = 1;
    private const uint ZClipDisableMask = 0x0C000000;
    private const int MaskExportSamplesShift = 8;

    // D3D12 enums.
    private const int FillWireframe = 2;
    private const int FillSolid = 3;
    private const int CullNone = 1;
    private const int CullFront = 2;
    private const int CullBack = 3;
    private const int ComparisonBase = 1;
    private const int SlopeUnits = 16;

    // BlendOp (gfx10) -> D3D12_BLEND; 0x0B and 0x0C (BOTH_SRC_ALPHA, BOTH_INV_SRC_ALPHA) have no D3D12 factor.
    private static readonly int[] BlendFactors = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0, 0, 14, 15, 16, 17, 18, 19, 20, 21];

    // D3D12_BLEND colour factors and their alpha twins (SRC_COLOR -> SRC_ALPHA, ...), for the alpha channel.
    private static readonly Dictionary<int, int> AlphaTwins = new() { [3] = 5, [4] = 6, [9] = 7, [10] = 8, [16] = 18, [17] = 19 };

    // CombFunc -> D3D12_BLEND_OP: DST_PLUS_SRC ADD, SRC_MINUS_DST SUBTRACT, MIN, MAX, DST_MINUS_SRC REV_SUBTRACT.
    private static readonly int[] BlendOps = [1, 2, 4, 5, 3];

    // StencilOp -> D3D12_STENCIL_OP; ONES (2) and REPLACE_OP (4) have none.
    private static readonly int[] StencilOps = [1, 2, 0, 3, 0, 4, 5, 6, 7, 8];

    // VGT_GS_OUTPRIM_TYPE -> D3D12_PRIMITIVE_TOPOLOGY_TYPE: points, lines, triangles.
    private static readonly int[] Topologies = [1, 2, 3];

    // BlendOp values of the SRC1 factors (gfx10): SRC1_COLOR, INV_SRC1_COLOR, SRC1_ALPHA, INV_SRC1_ALPHA.
    private static readonly int[] SecondSourceFactors = [0x0F, 0x10, 0x11, 0x12];

    /// <summary>The D3D12 fixed-function state of the registers.</summary>
    public static FixedFunctionState Decode(GraphicsStateRegisters registers)
    {
        var blend = Enumerable.Range(0, GraphicsStateParser.Targets).Select(target => Blend(registers, target)).ToList();
        var dualSource = UsesSecondSource(registers.BlendControl[0]);
        var forced = 1 << (int)((registers.Eqaa >> MaskExportSamplesShift) & ThreeBits);

        if (forced == 1)
        {
            forced = 0;
        }

        return new FixedFunctionState(
            blend,
            dualSource,
            Targets(registers, dualSource),
            DepthStencil(registers),
            Rasterizer(registers),
            Lookup(Topologies, registers.GsOutPrimType),
            forced);
    }

    private static BlendTarget Blend(GraphicsStateRegisters registers, int target)
    {
        var control = registers.BlendControl[target];
        var colorSrc = Field(control, ColorSrcShift, FiveBits);
        var colorDest = Field(control, ColorDestShift, FiveBits);
        var colorComb = Field(control, ColorCombShift, ThreeBits);
        var alphaSrc = colorSrc;

        var alphaDest = colorDest;
        var alphaComb = colorComb;

        if (Bit(control, SeparateAlphaBit))
        {
            alphaSrc = Field(control, AlphaSrcShift, FiveBits);
            alphaDest = Field(control, AlphaDestShift, FiveBits);
            alphaComb = Field(control, AlphaCombShift, ThreeBits);
        }

        return new BlendTarget(
            Bit(control, EnableBit),
            Lookup(BlendFactors, colorSrc),
            Lookup(BlendFactors, colorDest),
            Lookup(BlendOps, colorComb),
            AlphaFactor(alphaSrc),
            AlphaFactor(alphaDest),
            Lookup(BlendOps, alphaComb),
            (int)Field(registers.TargetMask, target * NibbleBits, Nibble));
    }

    private static int AlphaFactor(uint hardware)
    {
        var factor = Lookup(BlendFactors, hardware);

        if (AlphaTwins.TryGetValue(factor, out var twin))
        {
            return twin;
        }

        return factor;
    }

    private static bool UsesSecondSource(uint control)
    {
        if (!Bit(control, EnableBit))
        {
            return false;
        }

        int[] shifts = [ColorSrcShift, ColorDestShift, AlphaSrcShift, AlphaDestShift];

        return shifts.Any(shift => SecondSourceFactors.Contains((int)Field(control, shift, FiveBits)));
    }

    // Exports are compacted: the n-th non-zero SPI_SHADER_COL_FORMAT entry belongs to the n-th target of CB_SHADER_MASK.
    private static List<RenderTargetSlot> Targets(GraphicsStateRegisters registers, bool dualSource)
    {
        var exports = Enumerable.Range(0, GraphicsStateParser.Targets)
            .Select(slot => (int)Field(registers.ColFormat, slot * NibbleBits, Nibble))
            .Where(format => format != 0)
            .ToList();
        var targets = new List<RenderTargetSlot>();

        if (exports.Count == 0)
        {
            return targets;
        }

        for (var slot = 0; slot < GraphicsStateParser.Targets; slot++)
        {
            if (Field(registers.ShaderMask, slot * NibbleBits, Nibble) == 0)
            {
                continue;
            }

            var export = exports.ElementAtOrDefault(targets.Count);
            var downconvert = (int)Field(registers.Downconvert, slot * NibbleBits, Nibble);
            targets.Add(new RenderTargetSlot(slot, RenderTargetFormats.Guess(downconvert, export), downconvert, export));
        }

        if (dualSource)
        {
            targets.RemoveAll(target => target.Index != 0);
        }

        return targets;
    }

    private static DepthStencilState DepthStencil(GraphicsStateRegisters registers)
    {
        var depth = registers.DepthControl;
        var stencil = registers.StencilControl;
        var front = Face(stencil, Field(depth, StencilFuncShift, ThreeBits));
        var back = front;

        if (Bit(depth, BackfaceBit))
        {
            back = Face(stencil >> StencilBackShift, Field(depth, StencilFuncBackShift, ThreeBits));
        }

        return new DepthStencilState(
            Bit(depth, ZEnableBit),
            Bit(depth, ZWriteBit),
            (int)Field(depth, ZFuncShift, ThreeBits) + ComparisonBase,
            Bit(depth, DepthBoundsBit),
            Bit(depth, StencilEnableBit),
            (int)Field(registers.StencilRefMask, StencilMaskShift, ByteMask),
            (int)Field(registers.StencilRefMask, StencilWriteMaskShift, ByteMask),
            front,
            back);
    }

    private static StencilFace Face(uint control, uint func)
    {
        return new StencilFace(
            Lookup(StencilOps, Field(control, StencilFailShift, Nibble)),
            Lookup(StencilOps, Field(control, StencilDepthFailShift, Nibble)),
            Lookup(StencilOps, Field(control, StencilPassShift, Nibble)),
            (int)func + ComparisonBase);
    }

    private static RasterizerState Rasterizer(GraphicsStateRegisters registers)
    {
        var mode = registers.ModeControl;
        var fill = FillSolid;

        if (Field(mode, PolyModeShift, PolyModeMask) != 0)
        {
            fill = 0;

            if (Field(mode, FrontPtypeShift, ThreeBits) == LinesPtype)
            {
                fill = FillWireframe;
            }
        }

        return new RasterizerState(
            fill,
            Cull(Bit(mode, CullFrontBit), Bit(mode, CullBackBit)),
            !Bit(mode, FaceBit),
            (int)registers.PolyOffsetOffset,
            registers.PolyOffsetClamp,
            registers.PolyOffsetScale / SlopeUnits,
            (registers.ClipControl & ZClipDisableMask) == 0);
    }

    private static int Cull(bool front, bool back)
    {
        if (front && back)
        {
            return 0;
        }

        if (front)
        {
            return CullFront;
        }

        if (back)
        {
            return CullBack;
        }

        return CullNone;
    }

    private static int Lookup(int[] table, uint index)
    {
        if (index >= table.Length)
        {
            return 0;
        }

        return table[index];
    }

    private static uint Field(uint value, int shift, uint mask)
    {
        return (value >> shift) & mask;
    }

    private static bool Bit(uint value, int bit)
    {
        return ((value >> bit) & 1) != 0;
    }
}
