// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// The resource binding of the immediate-mode textured unlit pass (VS_TransformUnlit_IM + PS_TexturedUnlit_IM), as
/// both programs read it. User SGPR s8 is the low half of the address of a descriptor table (s9, the high half, is
/// set to 0 by the code) in the GPU-memory window (heap slot 0); the programs load from it:
/// <list type="table">
/// <item><term>+0x00</term><description>PS: V# of a buffer; dword 0 scales alpha (squared), dword 3 scales colour.</description></item>
/// <item><term>+0x20</term><description>VS: V# of the object constants: a 4x4 matrix of row vectors (rows 0-2 multiply x, y, z, row 3 adds).</description></item>
/// <item><term>+0x40</term><description>PS: T# of the texture, sampled with a literal S# (anisotropic, wrap).</description></item>
/// <item><term>+0x60</term><description>VS: V# of the view constants; the 4x4 view-projection matrix (row vectors) at byte 0x160.</description></item>
/// </list>
/// Descriptor dword 0 is a <c>ResourceDescriptorHeap</c> index. Both stages see the same user data (one root CBV), so
/// they share one table; their offsets do not overlap. A draw with several textures gets one table per texture, the
/// k-th naming heap slot <see cref="TextureSlot"/> + k; the buffers are shared.
/// </summary>
public static class UnlitBindings
{
    public const int WindowSlot = 0;
    public const int PixelConstantsSlot = 2;
    public const int ObjectConstantsSlot = 3;
    public const int ViewConstantsSlot = 4;
    public const int TextureSlot = 5;

    /// <summary>User SGPR holding the table address.</summary>
    public const int TableSgpr = 8;

    /// <summary>The first table's byte address in the GPU-memory window (any 16-byte aligned value works).</summary>
    public const int TableAddress = 0x100;

    /// <summary>The distance between consecutive tables.</summary>
    public const int TableStride = 0x80;

    public const int UserDataBytes = 512;
    public const int PixelConstantsBytes = 16;
    public const int ObjectConstantsBytes = 64;
    public const int ViewMatrixOffset = 0x160;
    public const int ViewConstantsBytes = ViewMatrixOffset + MatrixBytes;

    private const int PixelBufferEntry = 0x00;
    private const int ObjectBufferEntry = 0x20;
    private const int TextureEntry = 0x40;
    private const int ViewBufferEntry = 0x60;
    private const int RecordsWord = 2;
    private const int ColourScaleWord = 3;
    private const int MatrixBytes = 64;
    private const int RowFloats = 4;
    private const float ClipSpan = 2f;

    /// <summary>The heap slots a draw with this many textures uses.</summary>
    public static int ResourceSlots(int textures)
    {
        return TextureSlot + textures;
    }

    /// <summary>The size of the GPU-memory window holding this many tables.</summary>
    public static int WindowBytes(int textures)
    {
        return TableAddress + (textures * TableStride);
    }

    /// <summary>The GPU-memory window: one descriptor table per texture from <see cref="TableAddress"/>, zeros elsewhere.</summary>
    public static byte[] Window(int textures)
    {
        var window = new byte[WindowBytes(textures)];

        for (var table = 0; table < textures; table++)
        {
            var start = Table(table);
            BufferDescriptor(window, start + PixelBufferEntry, PixelConstantsSlot, PixelConstantsBytes);
            BufferDescriptor(window, start + ObjectBufferEntry, ObjectConstantsSlot, ObjectConstantsBytes);
            BufferDescriptor(window, start + ViewBufferEntry, ViewConstantsSlot, ViewConstantsBytes);
            Word(window, start + TextureEntry, TextureSlot + table);
        }

        return window;
    }

    /// <summary>The <c>UserData</c> constant buffer (128 dwords) of the draws with one texture: s8 = its table's address.</summary>
    public static byte[] UserData(int table)
    {
        var data = new byte[UserDataBytes];
        Word(data, TableSgpr * sizeof(uint), Table(table));

        return data;
    }

    /// <summary>The pixel shader's buffer: alpha scale (squared by the code) and colour scale.</summary>
    public static byte[] PixelConstants(float alphaScale, float colourScale)
    {
        var data = new byte[PixelConstantsBytes];
        BinaryPrimitives.WriteSingleLittleEndian(data, alphaScale);
        BinaryPrimitives.WriteSingleLittleEndian(data.AsSpan(ColourScaleWord * sizeof(float)), colourScale);

        return data;
    }

    /// <summary>The object constants: the identity, so positions reach the view matrix unchanged.</summary>
    public static byte[] ObjectConstants()
    {
        var data = new byte[ObjectConstantsBytes];

        for (var row = 0; row < RowFloats; row++)
        {
            BinaryPrimitives.WriteSingleLittleEndian(data.AsSpan(((row * RowFloats) + row) * sizeof(float)), 1);
        }

        return data;
    }

    /// <summary>
    /// The view constants: a view-projection that maps pixel coordinates (y down) to clip space and keeps z, so the
    /// vertex's z is its stored depth.
    /// </summary>
    public static byte[] ViewConstants(int width, int height)
    {
        var data = new byte[ViewConstantsBytes];
        float[] matrix =
        [
            ClipSpan / width, 0, 0, 0,
            0, -ClipSpan / height, 0, 0,
            0, 0, 1, 0,
            -1, 1, 0, 1,
        ];

        for (var index = 0; index < matrix.Length; index++)
        {
            BinaryPrimitives.WriteSingleLittleEndian(data.AsSpan(ViewMatrixOffset + (index * sizeof(float))), matrix[index]);
        }

        return data;
    }

    private static int Table(int table)
    {
        return TableAddress + (table * TableStride);
    }

    /// <summary>A V#: dword 0 the heap slot, dword 2 the size in bytes (NUM_RECORDS of a raw buffer).</summary>
    private static void BufferDescriptor(byte[] window, int entry, int slot, int bytes)
    {
        Word(window, entry, slot);
        Word(window, entry + (RecordsWord * sizeof(uint)), bytes);
    }

    private static void Word(byte[] data, int offset, int value)
    {
        BinaryPrimitives.WriteInt32LittleEndian(data.AsSpan(offset), value);
    }
}
