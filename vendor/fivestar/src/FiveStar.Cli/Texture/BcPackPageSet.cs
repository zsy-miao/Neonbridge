// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Ttpf;
using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Cli.Texture;

/// <summary>The BCPack pages of one codec in a set of page files, and the decoder engine and codec the command asked for.</summary>
internal static class BcPackPageSet
{
    /// <summary>Output bytes per lane symbol, in every codec.</summary>
    public const int BytesPerSymbol = 8;

    private const string PagePattern = "*.ttpf";

    /// <summary>Every page file given or below a folder given, sorted.</summary>
    public static List<string> Files(IReadOnlyList<string> paths)
    {
        var files = new List<string>();

        foreach (var path in paths)
        {
            if (Directory.Exists(path))
            {
                files.AddRange(Directory.EnumerateFiles(path, PagePattern, SearchOption.AllDirectories));
            }
            else if (File.Exists(path))
            {
                files.Add(path);
            }
            else
            {
                throw new FileNotFoundException("no such page file or folder: " + path);
            }
        }

        files.Sort(StringComparer.OrdinalIgnoreCase);

        return files;
    }

    /// <summary>The pages of <paramref name="codec"/> in each file, every <paramref name="every"/>-th one per file.</summary>
    public static List<(int File, TexturePage Page)> Pages(IReadOnlyList<string> files, long every, BcPackCodec codec)
    {
        var pages = new List<(int File, TexturePage Page)>();

        for (var index = 0; index < files.Count; index++)
        {
            using var source = new FileByteSource(files[index]);
            var file = new TexturePageFile(source, new ZlibInflater());
            long seen = 0;

            foreach (var page in file.Pages)
            {
                if (page.IsBcPack && page.Codec == (int)codec && seen++ % every == 0)
                {
                    pages.Add((index, page));
                }
            }
        }

        return pages;
    }

    /// <summary>The codec named by <c>--codec 1|3|4|5|6|7</c>, BC1 by default.</summary>
    public static BcPackCodec Codec(string? number)
    {
        if (number is null)
        {
            return BcPackCodec.Bc1;
        }

        if (!Enum.TryParse<BcPackCodec>(number, out var codec) || !Enum.IsDefined(codec))
        {
            throw new ArgumentException($"unknown codec {number} (1, 3, 4, 5, 6, 7)");
        }

        return codec;
    }

    /// <summary>The engine named by <c>--engine avx512|avx2|scalar</c>, or the fastest one this processor runs.</summary>
    public static BcPackEngine Engine(string? name)
    {
        if (name is null)
        {
            return BcPackDecoder.DefaultEngine;
        }

        if (!Enum.TryParse<BcPackEngine>(name, ignoreCase: true, out var engine) || !Enum.IsDefined(engine))
        {
            throw new ArgumentException($"unknown engine {name} (avx512, avx2, scalar)");
        }

        if (!BcPackDecoder.IsSupported(engine))
        {
            throw new ArgumentException($"this processor cannot run the {name} engine");
        }

        return engine;
    }

    public static string CodecName(BcPackCodec codec) => codec.ToString().ToUpperInvariant();

    public static string EngineName(BcPackEngine engine)
    {
        switch (engine)
        {
            case BcPackEngine.Avx512:
                return "AVX-512";
            case BcPackEngine.Avx2:
                return "AVX2";
            default:
                return "scalar";
        }
    }
}
