// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The hardware dword 3 a typed buffer view's V# carries (RDNA 2 ISA, buffer resource): DST_SEL_X..W in bits 0-11, the
/// gfx10 unified buffer format in bits 12-18 and RESOURCE_LEVEL (bit 24, 1 on gfx10). Translated programs decode it as
/// they decode a V# the game builds itself (rdna_prelude buf_code, FmtLoad), so a B8G8R8A8 view reads R in x.
/// </summary>
internal static class BufferFormatCodes
{
    /// <summary>No format: formatted loads read zeros.</summary>
    public const uint None = 0;

    private const int FormatShift = 12;
    private const uint ResourceLevel = 1u << 24;

    /// <summary>DST_SEL: x, y, z, w as loaded; and z, y, x, w (BGRA); and z, y, x, 1 (BGRX).</summary>
    private const uint Xyzw = 0xFAC;
    private const uint Zyxw = 0xF2E;
    private const uint Zyx1 = 0x32E;

    // Unified formats (RDNA 2 ISA, buffer data formats).
    private const uint U8Unorm = 1;
    private const uint U8Snorm = 2;
    private const uint U8Uint = 5;
    private const uint U8Sint = 6;
    private const uint U16Unorm = 7;
    private const uint U16Snorm = 8;
    private const uint U16Uint = 11;
    private const uint U16Sint = 12;
    private const uint U16Float = 13;
    private const uint U8x2Unorm = 14;
    private const uint U8x2Snorm = 15;
    private const uint U8x2Uint = 18;
    private const uint U8x2Sint = 19;
    private const uint U32Uint = 20;
    private const uint U32Sint = 21;
    private const uint U32Float = 22;
    private const uint U16x2Unorm = 23;
    private const uint U16x2Snorm = 24;
    private const uint U16x2Uint = 27;
    private const uint U16x2Sint = 28;
    private const uint U16x2Float = 29;
    private const uint U10x11x11Float = 36;
    private const uint U2x10x10x10Unorm = 50;
    private const uint U2x10x10x10Uint = 54;
    private const uint U8x4Unorm = 56;
    private const uint U8x4Snorm = 57;
    private const uint U8x4Uint = 60;
    private const uint U8x4Sint = 61;
    private const uint U32x2Uint = 62;
    private const uint U32x2Sint = 63;
    private const uint U32x2Float = 64;
    private const uint U16x4Unorm = 65;
    private const uint U16x4Snorm = 66;
    private const uint U16x4Uint = 69;
    private const uint U16x4Sint = 70;
    private const uint U16x4Float = 71;
    private const uint U32x3Uint = 72;
    private const uint U32x3Sint = 73;
    private const uint U32x3Float = 74;
    private const uint U32x4Uint = 75;
    private const uint U32x4Sint = 76;
    private const uint U32x4Float = 77;

    /// <summary>32_UINT: one dword as stored, for raw and structured views.</summary>
    public const uint Dword = Xyzw | (U32Uint << FormatShift) | ResourceLevel;

    /// <summary>32_32_32_32_FLOAT: a constant buffer's float4 rows.</summary>
    public const uint Float4 = Xyzw | (U32x4Float << FormatShift) | ResourceLevel;

    /// <summary>DXGI_FORMAT (dxgiformat.h), its unified format and destination swizzle.</summary>
    private static readonly (int Format, uint Unified, uint Select)[] Formats =
    [
        (2, U32x4Float, Xyzw), (3, U32x4Uint, Xyzw), (4, U32x4Sint, Xyzw),
        (6, U32x3Float, Xyzw), (7, U32x3Uint, Xyzw), (8, U32x3Sint, Xyzw),
        (10, U16x4Float, Xyzw), (11, U16x4Unorm, Xyzw), (12, U16x4Uint, Xyzw), (13, U16x4Snorm, Xyzw), (14, U16x4Sint, Xyzw),
        (16, U32x2Float, Xyzw), (17, U32x2Uint, Xyzw), (18, U32x2Sint, Xyzw),
        (24, U2x10x10x10Unorm, Xyzw), (25, U2x10x10x10Uint, Xyzw), (26, U10x11x11Float, Xyzw),
        (28, U8x4Unorm, Xyzw), (29, U8x4Unorm, Xyzw), (30, U8x4Uint, Xyzw), (31, U8x4Snorm, Xyzw), (32, U8x4Sint, Xyzw),
        (34, U16x2Float, Xyzw), (35, U16x2Unorm, Xyzw), (36, U16x2Uint, Xyzw), (37, U16x2Snorm, Xyzw), (38, U16x2Sint, Xyzw),
        (39, U32Uint, Xyzw), (40, U32Float, Xyzw), (41, U32Float, Xyzw), (42, U32Uint, Xyzw), (43, U32Sint, Xyzw),
        (49, U8x2Unorm, Xyzw), (50, U8x2Uint, Xyzw), (51, U8x2Snorm, Xyzw), (52, U8x2Sint, Xyzw),
        (54, U16Float, Xyzw), (55, U16Unorm, Xyzw), (56, U16Unorm, Xyzw), (57, U16Uint, Xyzw), (58, U16Snorm, Xyzw), (59, U16Sint, Xyzw),
        (61, U8Unorm, Xyzw), (62, U8Uint, Xyzw), (63, U8Snorm, Xyzw), (64, U8Sint, Xyzw),
        (87, U8x4Unorm, Zyxw), (88, U8x4Unorm, Zyx1), (91, U8x4Unorm, Zyxw), (93, U8x4Unorm, Zyx1),
    ];

    private static readonly uint[] ByFormat = Table();

    /// <summary>The dword 3 of a DXGI_FORMAT; <see cref="None"/> for a format no buffer load decodes.</summary>
    [HotPath]
    public static uint Of(int format)
    {
        if ((uint)format >= (uint)ByFormat.Length)
        {
            return None;
        }

        return ByFormat[format];
    }

    private static uint[] Table()
    {
        var table = new uint[Formats.Max(entry => entry.Format) + 1];

        foreach (var (format, unified, select) in Formats)
        {
            table[format] = select | (unified << FormatShift) | ResourceLevel;
        }

        return table;
    }
}
