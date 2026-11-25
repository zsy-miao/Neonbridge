// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Formats.Ttpf;

/// <summary>
/// Decides which textures of a page file can be extracted without reading their pages: a texture counts as ok when its
/// layout is known and every page under its mip 0 can be decoded exactly (stored, zlib or BCPack BC1), and as
/// approximate when a page under it decodes only approximately (<see cref="TexturePage.IsApproximate"/>).
/// </summary>
public sealed class TextureCoverage
{
    private readonly TexturePageFile _file;
    private readonly TextureExtractor _extractor;

    public TextureCoverage(TexturePageFile file, TextureExtractor extractor)
    {
        _file = file;
        _extractor = extractor;
    }

    public CoverageReport Measure()
    {
        var textures = new Tally<string>();
        var spanBytes = new Tally<string>();
        var blockers = new Tally<(int Codec, string Format)>();

        foreach (var descriptor in _file.Descriptors)
        {
            var span = _extractor.SpanEnd(descriptor) - descriptor.VirtualAddress;
            var status = Classify(descriptor, blockers);
            textures.Add(status.Name);
            spanBytes.Add(status.Name, span);
        }

        return new CoverageReport(textures, spanBytes, blockers);
    }

    /// <summary>The codec of the first undecodable BCPack page overlapping [start, start + size), or null when there is none.</summary>
    public int? BlockingCodec(long start, long size) => FirstCodec(start, size, page => !page.IsDecodable);

    /// <summary>The codec of the first approximate BCPack page overlapping [start, start + size), or null when there is none.</summary>
    public int? ApproximateCodec(long start, long size) => FirstCodec(start, size, page => page.IsApproximate);

    private int? FirstCodec(long start, long size, Func<TexturePage, bool> match)
    {
        var index = Math.Max(_file.PageAt(start), 0);

        while (index < _file.Pages.Count && _file.Pages[index].VirtualAddress < start + size)
        {
            var page = _file.Pages[index];

            if (match(page) && page.VirtualAddress + page.EffectiveVirtualSize > start)
            {
                return page.Codec;
            }

            index++;
        }

        return null;
    }

    private TextureStatus Classify(TextureDescriptor descriptor, Tally<(int Codec, string Format)> blockers)
    {
        var (status, layout) = _extractor.Locate(descriptor);

        if (layout is null)
        {
            return status;
        }

        for (var image = 0; image < layout.Images; image++)
        {
            var codec = BlockingCodec(layout.Start + (image * layout.ImageStride), layout.Size);

            if (codec.HasValue)
            {
                blockers.Add((codec.Value, layout.Format.Name));

                return new TextureStatus(TextureStatusKind.BcPack, codec.Value);
            }
        }

        for (var image = 0; image < layout.Images; image++)
        {
            var codec = ApproximateCodec(layout.Start + (image * layout.ImageStride), layout.Size);

            if (codec.HasValue)
            {
                return new TextureStatus(TextureStatusKind.Approximate, codec.Value);
            }
        }

        return status;
    }
}
