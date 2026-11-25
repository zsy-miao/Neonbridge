// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Exe;

/// <summary>Builds the <see cref="ExeAnalysis"/> of one executable from the PE readers, the loader and the GDK library.</summary>
internal static class ExeAnalyzer
{
    private static readonly string[] DefaultArchiveNames =
    [
        "anim.rpf", "audio.rpf", "common.rpf", "data.rpf", "levels.rpf", "movies.rpf", "navmeshes.rpf",
        "packs.rpf", "script.rpf", "shaders.rpf", "textures.rpf", "appdata0.rpf",
    ];

    public static ExeAnalysis Analyze(AnalyzeOptions options)
    {
        var image = new PeImage(File.ReadAllBytes(options.ExePath));
        var basics = PeFacts.Build(image);
        var sites = ImportCallSites.Count(image, AllImports(image));
        var gdkTallies = ScanGdk(image, options.GdkLib);
        var verdicts = Classify(options, sites, gdkTallies);

        var imports = ImportAudit.Build(sites, verdicts);
        var gdkCalls = GdkAudit.ToCalls(gdkTallies, verdicts);
        var graphics = GraphicsAudit.Build(imports);
        var archiveReferences = ReferenceScan.ArchiveReferences(image, ArchiveNames(options.DumpFolder));
        var shaderReferences = ReferenceScan.ShaderReferences(image);
        var dayOne = DayOne.Build(graphics, gdkCalls, imports, archiveReferences);

        return Assemble(options, image, basics, imports, verdicts, gdkCalls, gdkTallies, graphics, archiveReferences, shaderReferences, dayOne);
    }

    private static IReadOnlyList<PeImport> AllImports(PeImage image)
    {
        var all = new List<PeImport>(PeImportReader.Imports(image));
        all.AddRange(PeImportReader.DelayImports(image));

        return all;
    }

    private static IReadOnlyList<GdkThunkTally> ScanGdk(PeImage image, string? gdkLib)
    {
        // The thunks are found without the library; the library only names them.
        if (gdkLib is null)
        {
            return GdkAudit.Scan(image, null);
        }

        return GdkAudit.Scan(image, File.ReadAllBytes(gdkLib));
    }

    private static IReadOnlyDictionary<string, LoaderVerdict>? Classify(AnalyzeOptions options, IReadOnlyList<ImportCallSite> sites, IReadOnlyList<GdkThunkTally> gdkTallies)
    {
        var loaderPath = LoaderLocator.Locate(options.LoaderPath);

        if (loaderPath is null)
        {
            return null;
        }

        var queries = new HashSet<string>(ImportAudit.Queries(sites), StringComparer.Ordinal);
        queries.UnionWith(GdkAudit.NameQueries(gdkTallies));

        var packageDir = Path.GetDirectoryName(Path.GetFullPath(options.ExePath)) ?? Environment.CurrentDirectory;

        return LoaderClassifier.Classify(loaderPath, queries, options.OutDir, packageDir);
    }

    private static IReadOnlyList<string> ArchiveNames(string? dumpFolder)
    {
        if (dumpFolder is null || !Directory.Exists(dumpFolder))
        {
            return DefaultArchiveNames;
        }

        var names = Directory.GetFiles(dumpFolder, "*.rpf").Select(Path.GetFileName).OfType<string>().ToList();

        if (names.Count == 0)
        {
            return DefaultArchiveNames;
        }

        return names;
    }

    private static ExeAnalysis Assemble(
        AnalyzeOptions options,
        PeImage image,
        PeBasics basics,
        IReadOnlyList<ImportGap> imports,
        IReadOnlyDictionary<string, LoaderVerdict>? verdicts,
        IReadOnlyList<GdkCall> gdkCalls,
        IReadOnlyList<GdkThunkTally> gdkTallies,
        IReadOnlyList<ImportGap> graphics,
        IReadOnlyList<SymbolReference> archiveReferences,
        IReadOnlyList<SymbolReference> shaderReferences,
        IReadOnlyList<GapItem> dayOne)
    {
        var named = gdkTallies.Count(tally => tally.Name is not null);
        string? gdkLibName = null;

        if (options.GdkLib is not null)
        {
            gdkLibName = Path.GetFileName(options.GdkLib);
        }

        return new ExeAnalysis
        {
            FileName = Path.GetFileName(options.ExePath),
            FileLength = new FileInfo(options.ExePath).Length,
            Is64Bit = image.Is64Bit,
            Machine = image.Machine,
            EntryPoint = image.EntryPoint,
            ImageBase = image.ImageBase,
            TimeStamp = image.TimeStamp,
            Subsystem = image.Subsystem,
            DllCharacteristics = image.DllCharacteristics,
            Sections = basics.Sections,
            TlsCallbacks = basics.TlsCallbacks,
            PdataFunctions = basics.PdataFunctions,
            PdataBytes = basics.PdataBytes,
            Exports = basics.Exports,
            Imports = imports,
            LoaderClassified = verdicts is not null,
            GdkCalls = gdkCalls,
            GdkThunkCount = gdkTallies.Count,
            GdkNamedCount = named,
            GdkLibName = gdkLibName,
            GraphicsImports = graphics,
            ArchiveReferences = archiveReferences,
            ShaderReferences = shaderReferences,
            DayOneGaps = dayOne,
        };
    }
}
