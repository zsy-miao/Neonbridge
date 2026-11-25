// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Texture;
using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Imports the user's BCPack tables (<c>--bcpack-tables &lt;file&gt;</c> or <c>FIVESTAR_BCPACK_TABLES</c>) into
/// <c>&lt;work&gt;/loader/bcpack-tables.bin</c>, after checking size and hash. The texture export and the loader read
/// that copy. The project does not ship the tables; without them the step finishes with a note.
/// </summary>
internal sealed class BcPackTablesStep : ISetupStep
{
    public const string StepName = "bcpack-tables";

    private const string OptionName = "bcpack-tables";

    private static readonly TimeSpan TypicalTime = TimeSpan.FromSeconds(1);

    public string Name => StepName;

    public string Title => "Import the BCPack tables";

    public IReadOnlyList<string> DependsOn => [];

    public TimeSpan Estimate(SetupContext context) => TypicalTime;

    public StepFingerprint Fingerprint(SetupContext context)
    {
        var source = Source(context) ?? context.BcPackTables;

        return new StepFingerprint(Fingerprints.OfContent(source), Fingerprints.OfAssemblies(typeof(BcPackTables)));
    }

    public bool HasOutputs(SetupContext context) => Source(context) is null || File.Exists(context.BcPackTables);

    public long RequiredBytes(SetupContext context) => BcPackTables.FileSize;

    public string? MissingRequirement(SetupContext context) => null;

    public StepResult Run(SetupContext context)
    {
        var source = Source(context);

        if (source is null)
        {
            if (File.Exists(context.BcPackTables))
            {
                return StepResult.Done($"kept {context.BcPackTables}");
            }

            return StepResult.Done($"skipped: no tables given (--bcpack-tables <file>); {BcPackTableSource.Fallback}");
        }

        if (!File.Exists(source))
        {
            return StepResult.Failed($"{source} does not exist");
        }

        var problem = BcPackTables.Check(File.ReadAllBytes(source));

        if (problem is not null)
        {
            return StepResult.Failed($"{source}: {problem}");
        }

        Directory.CreateDirectory(context.LoaderFolder);
        File.Copy(source, context.BcPackTables, overwrite: true);

        return StepResult.Done($"{context.BcPackTables} from {source}: every BCPack codec decodes exactly");
    }

    /// <summary>The file the user names, or null.</summary>
    private static string? Source(SetupContext context)
    {
        var named = BcPackTableSource.Named(context.Arguments.Option(OptionName));

        if (named is null)
        {
            return null;
        }

        return Path.GetFullPath(named);
    }
}
