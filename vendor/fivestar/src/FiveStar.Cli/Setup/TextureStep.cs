// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Cli.Shader;
using FiveStar.Cli.Texture;
using FiveStar.Formats.Ttpf;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Exports mip 0 of every readable texture as DDS, one folder per page file below <c>&lt;work&gt;/textures</c> (runs
/// <c>fivestar texture extract</c> per file). Each finished file leaves a mark with its fingerprint, so an interrupted
/// run resumes at the first file without a current mark. With the imported BCPack tables every codec exports.
/// </summary>
internal sealed class TextureStep : ISetupStep
{
    public const string StepName = "textures";

    private const string PagePattern = "*.ttpf";
    private const string MarkFolder = "textures";
    private const string MarkExtension = ".done";
    private const int BytesPerMebibyte = 1 << 20;

    // Measured: the DDS files take 0.9 to 2.2 times their page file (mip 0 only).
    private const double SpaceFactor = 2;

    // One sampled texture is assumed to take at most 4 MiB.
    private const long BytesPerSampledTexture = 4L << 20;

    // Measured: about 100 MiB of page file per second; a sampled file takes well under a second.
    private const double PageBytesPerSecond = 100 << 20;

    public string Name => StepName;

    public string Title => "Export textures";

    public IReadOnlyList<string> DependsOn => [VerifyStep.StepName, BcPackTablesStep.StepName];

    public TimeSpan Estimate(SetupContext context)
    {
        var remaining = Remaining(context);

        if (context.Limit is not null)
        {
            return TimeSpan.FromSeconds(remaining.Count);
        }

        return TimeSpan.FromSeconds(remaining.Sum(file => new FileInfo(file).Length) / PageBytesPerSecond);
    }

    /// <summary>Every texture page file of the dump, in ordinal path order.</summary>
    public static IReadOnlyList<string> PageFiles(string dump)
    {
        if (!Directory.Exists(dump))
        {
            return [];
        }

        var files = Directory.GetFiles(dump, PagePattern, SearchOption.AllDirectories);
        Array.Sort(files, StringComparer.Ordinal);

        return files;
    }

    public StepFingerprint Fingerprint(SetupContext context)
    {
        var inputs = Fingerprints.Combine(Fingerprints.OfFiles(context.Dump, PageFiles(context.Dump)), context.Sample, Fingerprints.OfContent(context.BcPackTables));

        return new StepFingerprint(inputs, Tool());
    }

    public bool HasOutputs(SetupContext context) => Directory.Exists(context.TextureFolder);

    public long RequiredBytes(SetupContext context)
    {
        var remaining = Remaining(context);

        if (context.Limit is not null)
        {
            return remaining.Count * context.Limit.Value * BytesPerSampledTexture;
        }

        return (long)(remaining.Sum(file => new FileInfo(file).Length) * SpaceFactor);
    }

    public string? MissingRequirement(SetupContext context) => null;

    public StepResult Run(SetupContext context)
    {
        var files = PageFiles(context.Dump);
        var remaining = Remaining(context);
        var total = (int)(remaining.Sum(file => new FileInfo(file).Length) / BytesPerMebibyte);
        var done = 0L;
        var progress = new ConsoleProgress(() => "MiB of page files");

        if (File.Exists(context.BcPackTables) && !BcPackTableSource.Install(context.BcPackTables, Console.Error))
        {
            return StepResult.Failed($"{context.BcPackTables} is not the BCPack table file: run the {BcPackTablesStep.StepName} step again");
        }

        foreach (var file in remaining)
        {
            Export(context, file);
            done += new FileInfo(file).Length;
            progress.Report(new CorpusProgress((int)(done / BytesPerMebibyte), Math.Max(1, total)));
        }

        return StepResult.Done($"{files.Count} page files exported to {context.TextureFolder} ({files.Count - remaining.Count} were already done)");
    }

    private static void Export(SetupContext context, string file)
    {
        var relative = Path.GetRelativePath(context.Dump, file);
        var output = Path.Combine(context.TextureFolder, Path.ChangeExtension(relative, null));

        if (Directory.Exists(output))
        {
            Directory.Delete(output, recursive: true);
        }

        var arguments = new List<string>();

        if (context.Limit is not null)
        {
            arguments.AddRange(["--limit", context.Limit.Value.ToString(CultureInfo.InvariantCulture)]);
        }

        new TextureReports(Console.Out).Extract(file, output, new Archive.CommandArguments(arguments, []));
        var mark = MarkPath(context, file);
        Directory.CreateDirectory(Path.GetDirectoryName(mark)!);
        File.WriteAllText(mark, Mark(context, file));
    }

    /// <summary>The page files without a current mark.</summary>
    private static List<string> Remaining(SetupContext context)
    {
        return PageFiles(context.Dump).Where(file => !IsCurrent(context, file)).ToList();
    }

    private static bool IsCurrent(SetupContext context, string file)
    {
        var mark = MarkPath(context, file);

        return File.Exists(mark) && File.ReadAllText(mark) == Mark(context, file);
    }

    private static string Mark(SetupContext context, string file)
    {
        return Fingerprints.Combine(Fingerprints.OfFiles(context.Dump, [file]), context.Sample, Fingerprints.OfContent(context.BcPackTables), Tool());
    }

    private static string MarkPath(SetupContext context, string file)
    {
        return Path.Combine(context.StateFolder, MarkFolder, Path.GetRelativePath(context.Dump, file) + MarkExtension);
    }

    private static string Tool()
    {
        // The page-file readers only: a change elsewhere in the command does not redo hours of export.
        return Fingerprints.OfAssemblies(typeof(TextureExtractor));
    }
}
