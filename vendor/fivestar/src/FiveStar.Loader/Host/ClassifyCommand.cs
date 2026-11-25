// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Host;

/// <summary>
/// The <c>--classify</c> mode: reads <c>module!symbol</c> lines, classifies each through the shim table and the
/// module facts (the <c>--search</c> directories are the package folder) and writes one
/// <c>kind&lt;TAB&gt;module!symbol&lt;TAB&gt;detail</c> line per import. It maps no image, so it is cheap and the
/// caller (the <c>fivestar exe analyze</c> report) does not duplicate the loader's shim list.
/// </summary>
internal static class ClassifyCommand
{
    private const char SymbolSeparator = '!';
    private const char OrdinalMarker = '#';
    private const char FieldSeparator = '\t';

    public static int Run(HostOptions options)
    {
        var log = new LoaderLog(TextWriter.Null, null, false);
        var heap = new CodeHeap();
        using var lastError = new LastErrorBridge(heap, log);
        var shims = new ShimTable(lastError, heap);
        var loaderOptions = HostComposition.CreateLoaderOptions(options);
        var classifier = new ImportClassifier(shims, loaderOptions.SearchDirectories, loaderOptions.ManualModules);

        var lines = File.ReadAllLines(options.ClassifyInput!);
        var output = new List<string>(lines.Length);

        foreach (var line in lines)
        {
            AddLine(classifier, line, output);
        }

        Write(options.ClassifyOutput, output);

        return LoaderHost.Success;
    }

    private static void AddLine(ImportClassifier classifier, string line, List<string> output)
    {
        var trimmed = line.Trim();

        if (trimmed.Length == 0)
        {
            return;
        }

        var bang = trimmed.IndexOf(SymbolSeparator, StringComparison.Ordinal);

        if (bang < 0)
        {
            return;
        }

        var module = trimmed[..bang];
        var symbol = trimmed[(bang + 1)..];
        var classification = Classify(classifier, module, symbol);
        output.Add($"{classification.Kind}{FieldSeparator}{module}{SymbolSeparator}{symbol}{FieldSeparator}{classification.Detail}");
    }

    private static ImportClassification Classify(ImportClassifier classifier, string module, string symbol)
    {
        if (symbol.StartsWith(OrdinalMarker))
        {
            var ordinal = uint.Parse(symbol[1..], CultureInfo.InvariantCulture);

            return classifier.Classify(module, null, ordinal);
        }

        return classifier.Classify(module, symbol, 0);
    }

    private static void Write(string? outputPath, IReadOnlyList<string> lines)
    {
        if (outputPath is null)
        {
            foreach (var line in lines)
            {
                Console.Out.WriteLine(line);
            }

            return;
        }

        File.WriteAllLines(outputPath, lines);
    }
}
