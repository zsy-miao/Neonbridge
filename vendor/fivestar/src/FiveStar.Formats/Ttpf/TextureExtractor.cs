// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// Locates and reads the mip 0 of each texture in a page file (docs/textures.md, Memory layout). A texture's span runs
/// from its descriptor address to the next descriptor's address (or the end of the last page); its mip chain is laid
/// out from the span's start: 2D textures and arrays level by level, cube maps as complete copies of the cube with the
/// full-resolution copy last, volumes in 3D bricks.
/// </summary>
public sealed class TextureExtractor
{
    private const int BaseMask = 7;
    private const int CubeFaces = 6;
    private const int LargeTileBase = 0;
    private const int LastSmallTileBase = 3;

    /// <summary>A cube copy larger than 128 KiB is padded to whole 64 KiB; smaller copies are not padded.</summary>
    private const long CopyPadThreshold = 0x20000;
    private const long CopyAlignment = 0x10000;

    private readonly TexturePageFile _file;
    private readonly Dictionary<long, long> _spanEnd = [];
    private readonly Dictionary<string, SwizzlePattern> _patterns = new(StringComparer.Ordinal);

    public TextureExtractor(TexturePageFile file)
    {
        _file = file;
        var starts = new SortedSet<long>();

        foreach (var descriptor in file.Descriptors)
        {
            starts.Add(descriptor.VirtualAddress);
        }

        long? previous = null;

        foreach (var start in starts)
        {
            if (previous.HasValue)
            {
                _spanEnd[previous.Value] = start;
            }

            previous = start;
        }

        if (file.Pages.Count > 0)
        {
            var last = file.Pages[^1];
            VirtualEnd = last.VirtualAddress + last.EffectiveVirtualSize;
        }
    }

    /// <summary>Gets the end of the last page: the span end of the last texture.</summary>
    public long VirtualEnd { get; }

    /// <summary>The end of a texture's span.</summary>
    public long SpanEnd(TextureDescriptor descriptor)
    {
        return _spanEnd.GetValueOrDefault(descriptor.VirtualAddress, VirtualEnd);
    }

    /// <summary>Where mip 0 is stored, or why that is not known.</summary>
    public (TextureStatus Status, TextureLayout? Layout) Locate(TextureDescriptor descriptor)
    {
        var format = TextureFormats.Find(descriptor.Format);

        if (format is null)
        {
            return (new TextureStatus(TextureStatusKind.UnsupportedFormat, descriptor.Format), null);
        }

        switch (descriptor.Kind)
        {
            case TextureKind.Texture2D:
            case TextureKind.Array:
                return LocateSlices(descriptor, format);
            case TextureKind.Cube:
                return LocateCube(descriptor, format);
            case TextureKind.Volume:
                return LocateVolume(descriptor, format);
            default:
                return (new TextureStatus(TextureStatusKind.UnsupportedKind, (int)descriptor.Kind), null);
        }
    }

    /// <summary>Mip 0 as row-major elements (cube faces or array slices one after another), or the reason it cannot be read.</summary>
    public (TextureStatus Status, byte[]? Data, TextureLayout? Layout) ReadMip0(TextureDescriptor descriptor)
    {
        var (status, layout) = Locate(descriptor);

        if (layout is null)
        {
            return (status, null, null);
        }

        var imageBytes = (long)layout.BlockWidth * layout.BlockHeight * layout.Depth * layout.Format.BytesPerElement;
        var data = new byte[imageBytes * layout.Images];

        for (var image = 0; image < layout.Images; image++)
        {
            var (tiled, missing) = _file.ReadVirtual(layout.Start + (image * layout.ImageStride), (int)layout.Size);

            if (missing != 0)
            {
                return (new TextureStatus(TextureStatusKind.UnsupportedCodec, 0), null, layout);
            }

            var linear = layout.Pattern.Unswizzle(tiled, layout.BlockWidth, layout.BlockHeight, layout.Format.BytesPerElement, layout.Depth);
            linear.CopyTo(data, imageBytes * image);
        }

        return (TextureStatus.Ok, data, layout);
    }

    /// <summary>A 2D texture or array: each unit of the chain holds that unit of every slice back to back.</summary>
    private (TextureStatus Status, TextureLayout? Layout) LocateSlices(TextureDescriptor descriptor, TextureFormat format)
    {
        var pattern = TilePattern(descriptor.LayoutMode & BaseMask, format);

        if (pattern is null)
        {
            return (new TextureStatus(TextureStatusKind.UnsupportedMode, descriptor.LayoutMode), null);
        }

        var chain = new MipChain(format, pattern, descriptor.Width, descriptor.Height, descriptor.MipCount);
        var slices = descriptor.Kind == TextureKind.Array ? descriptor.ArraySize : 1;

        if (descriptor.VirtualAddress + (chain.SliceSize * slices) > SpanEnd(descriptor))
        {
            return (new TextureStatus(TextureStatusKind.Size, 0), null);
        }

        var mip0 = chain.Mips[0];
        var shape = Mip0Shape(descriptor, format, pattern, chain);
        var start = descriptor.VirtualAddress + (mip0.UnitOffset * slices) + mip0.TailOffset;

        return (TextureStatus.Ok, shape with { Start = start, Images = slices, ImageStride = mip0.UnitSize });
    }

    /// <summary>
    /// A cube map of c = mode - 8 copies, smallest first: copy k holds the smallest units - c + 1 + k units of every
    /// face, face after face; the last copy is the full cube.
    /// </summary>
    private (TextureStatus Status, TextureLayout? Layout) LocateCube(TextureDescriptor descriptor, TextureFormat format)
    {
        var copies = descriptor.LayoutMode & BaseMask;
        var bits = SwizzlePatterns.Find(SwizzlePatterns.SmallTile, format.BytesPerElement);

        if (bits is null)
        {
            return (new TextureStatus(TextureStatusKind.UnsupportedMode, descriptor.LayoutMode), null);
        }

        var pattern = Pattern(bits);
        var chain = new MipChain(format, pattern, descriptor.Width, descriptor.Height, descriptor.MipCount);

        if (copies == 0 || copies > chain.Units)
        {
            return (new TextureStatus(TextureStatusKind.CubeCopies, 0), null);
        }

        long fullCopy = 0;

        for (var copy = 0; copy < copies - 1; copy++)
        {
            fullCopy += CopySize(chain.SmallestUnitsSize(chain.Units - copies + 1 + copy));
        }

        if (descriptor.VirtualAddress + fullCopy + CopySize(chain.SliceSize) > SpanEnd(descriptor))
        {
            return (new TextureStatus(TextureStatusKind.Size, 0), null);
        }

        var mip0 = chain.Mips[0];
        var start = descriptor.VirtualAddress + fullCopy + mip0.UnitOffset + mip0.TailOffset;

        return (TextureStatus.Ok, Mip0Shape(descriptor, format, pattern, chain) with { Start = start, Images = CubeFaces, ImageStride = chain.SliceSize });
    }

    /// <summary>A volume of one mip: 4 KB bricks in x, then y, then z.</summary>
    private (TextureStatus Status, TextureLayout? Layout) LocateVolume(TextureDescriptor descriptor, TextureFormat format)
    {
        var bits = SwizzlePatterns.FindVolume(format.BytesPerElement);

        if (bits is null || descriptor.MipCount > 1)
        {
            return (new TextureStatus(TextureStatusKind.UnsupportedMode, descriptor.LayoutMode), null);
        }

        var pattern = Pattern(bits);
        var width = Elements(descriptor.Width, format);
        var height = Elements(descriptor.Height, format);
        var size = pattern.PaddedSize(width, height, format.BytesPerElement, descriptor.ArraySize);

        if (descriptor.VirtualAddress + size > SpanEnd(descriptor))
        {
            return (new TextureStatus(TextureStatusKind.Size, 0), null);
        }

        return (TextureStatus.Ok, new TextureLayout(format, descriptor.Kind, width, height, descriptor.ArraySize, pattern, size, descriptor.VirtualAddress, 1, 0));
    }

    /// <summary>Mip 0 of one image; a mip 0 in the tail sits in its slot and uses the shortest pattern prefix covering it.</summary>
    private TextureLayout Mip0Shape(TextureDescriptor descriptor, TextureFormat format, SwizzlePattern pattern, MipChain chain)
    {
        var mip0 = chain.Mips[0];
        var used = pattern;
        var size = mip0.UnitSize;

        if (mip0.InTail)
        {
            used = Pattern(pattern.FitBits(mip0.BlockWidth, mip0.BlockHeight));
            size = used.PaddedSize(mip0.BlockWidth, mip0.BlockHeight, format.BytesPerElement);
        }

        return new TextureLayout(format, descriptor.Kind, mip0.BlockWidth, mip0.BlockHeight, 1, used, size, 0, 1, 0);
    }

    private static long CopySize(long faceChain)
    {
        var size = faceChain * CubeFaces;

        if (size <= CopyPadThreshold)
        {
            return size;
        }

        return (size + CopyAlignment - 1) / CopyAlignment * CopyAlignment;
    }

    private static int Elements(int pixels, TextureFormat format)
    {
        return (pixels + format.BlockDimension - 1) / format.BlockDimension;
    }

    private SwizzlePattern? TilePattern(int layoutBase, TextureFormat format)
    {
        string? bits = null;

        if (layoutBase == LargeTileBase)
        {
            bits = SwizzlePatterns.Find(SwizzlePatterns.LargeTile, format.BytesPerElement);
        }
        else if (layoutBase <= LastSmallTileBase)
        {
            bits = SwizzlePatterns.Find(SwizzlePatterns.SmallTile, format.BytesPerElement);
        }

        if (bits is null)
        {
            return null;
        }

        return Pattern(bits);
    }

    private SwizzlePattern Pattern(string bits)
    {
        if (!_patterns.TryGetValue(bits, out var pattern))
        {
            pattern = new SwizzlePattern(bits);
            _patterns[bits] = pattern;
        }

        return pattern;
    }
}
