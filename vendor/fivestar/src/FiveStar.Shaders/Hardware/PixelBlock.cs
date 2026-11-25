// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>Decodes the pixel-shader block of an FSSM header (hardware stage 5, 0x138 .. 0x2FF).</summary>
internal static class PixelBlock
{
    private const int InterpolantTableStart = 0x138;
    private const int InterpolantTableEnd = 0x238;
    private const int InterpolantEntrySize = 8;
    private const int InterpolantSecondWord = 4;
    private const int InterpolantCountField = 0x238;
    private const int Rsrc2Field = 0x23C;
    private const int InputEnableField = 0x248;
    private const int DbShaderControlField = 0x24C;
    private const int CbShaderMaskField = 0x250;

    // DB_SHADER_CONTROL: bit 0 Z_EXPORT_ENABLE, bit 6 KILL_ENABLE, bit 16 PRIMITIVE_ORDERED_PIXEL_SHADER.
    private const uint ZExportBit = 0x1;
    private const uint KillEnableBit = 0x40;
    private const int PopsShift = 16;

    // CB_SHADER_MASK: 4 bits per render target, 8 targets.
    private const int RenderTargets = 8;
    private const int BitsPerTarget = 4;
    private const uint TargetMask = 0xF;

    // SPI_PS_INPUT_ENA / ADDR bit order and VGPR count of each input (gfx10 register reference).
    private static readonly (string Name, int Vgprs)[] Inputs =
    [
        ("persp_sample", 2), ("persp_center", 2), ("persp_centroid", 2), ("persp_pull_model", 3),
        ("linear_sample", 2), ("linear_center", 2), ("linear_centroid", 2), ("line_stipple", 1),
        ("pos_x", 1), ("pos_y", 1), ("pos_z", 1), ("pos_w", 1), ("front_face", 1), ("ancillary", 1),
        ("sample_coverage", 1), ("pos_fixed_pt", 1),
    ];

    public static PixelConfig Decode(ReadOnlySpan<byte> program, int userSgprs)
    {
        var enable = FssmHeader.U32(program, InputEnableField);
        var inputs = Layout(enable);
        var control = FssmHeader.U32(program, DbShaderControlField);
        var pops = ((control >> PopsShift) & 1) != 0;
        var mask = FssmHeader.U32(program, CbShaderMaskField);
        int? popsSgpr = pops ? userSgprs + 1 : null;

        return new PixelConfig
        {
            Rsrc2 = FssmHeader.U32(program, Rsrc2Field),
            InterpolantCount = (int)FssmHeader.U32(program, InterpolantCountField),
            InputEnable = enable,
            Inputs = inputs,
            InputVgprCount = inputs.Sum(input => input.Vgprs.Count),
            PrimMaskSgpr = userSgprs,
            DbShaderControl = control,
            Pops = pops,
            PopsWaveIdSgpr = popsSgpr,
            ZExport = (control & ZExportBit) != 0,
            KillEnable = (control & KillEnableBit) != 0,
            CbShaderMask = mask,
            MrtCount = MrtCount(mask),
            InterpolantTable = InterpolantTable(program),
        };
    }

    /// <summary>Every enabled input with its VGPRs, packed from v0 in bit order (ENA equals ADDR in these programs).</summary>
    public static IReadOnlyList<PixelInput> Layout(uint enable)
    {
        var layout = new List<PixelInput>();
        var next = 0;

        for (var bit = 0; bit < Inputs.Length; bit++)
        {
            if (((enable >> bit) & 1) == 0)
            {
                continue;
            }

            var (name, count) = Inputs[bit];
            layout.Add(new PixelInput(name, Enumerable.Range(next, count).ToList()));
            next += count;
        }

        return layout;
    }

    private static int MrtCount(uint mask)
    {
        var count = 0;

        for (var target = 0; target < RenderTargets; target++)
        {
            if (((mask >> (BitsPerTarget * target)) & TargetMask) != 0)
            {
                count = target + 1;
            }
        }

        return count;
    }

    private static List<(uint First, uint Second)> InterpolantTable(ReadOnlySpan<byte> program)
    {
        var table = new List<(uint First, uint Second)>();

        for (var at = InterpolantTableStart; at < InterpolantTableEnd; at += InterpolantEntrySize)
        {
            var first = FssmHeader.U32(program, at);
            var second = FssmHeader.U32(program, at + InterpolantSecondWord);

            if (first != 0 || second != 0)
            {
                table.Add((first, second));
            }
        }

        return table;
    }
}
