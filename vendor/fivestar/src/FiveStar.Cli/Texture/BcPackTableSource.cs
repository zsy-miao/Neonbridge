// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Cli.Texture;

/// <summary>
/// Where the commands find the user's BCPack tables (<see cref="BcPackTables"/>): <c>--bcpack-tables &lt;file&gt;</c>,
/// else the <c>FIVESTAR_BCPACK_TABLES</c> variable, else the copy <c>fivestar setup</c> imported into the work folder.
/// </summary>
internal static class BcPackTableSource
{
    public const string Option = "--bcpack-tables";

    public const string Variable = "FIVESTAR_BCPACK_TABLES";

    /// <summary>What decodes without the tables, for messages.</summary>
    public const string Fallback = "BC3, BC6H and BC7 pages stay undecoded, BC4 and BC5 decode approximately";

    /// <summary>The file named by the option or the variable, or null.</summary>
    public static string? Named(string? option)
    {
        if (!string.IsNullOrWhiteSpace(option))
        {
            return option;
        }

        var variable = Environment.GetEnvironmentVariable(Variable);

        if (string.IsNullOrWhiteSpace(variable))
        {
            return null;
        }

        return variable;
    }

    /// <summary>
    /// Installs the tables of <paramref name="path"/> for this process; false, with the reason on
    /// <paramref name="error"/>, when the file is missing or not the known table file. A null path installs nothing.
    /// </summary>
    public static bool Install(string? path, TextWriter error)
    {
        if (path is null)
        {
            return true;
        }

        try
        {
            BcPackPageDecoder.Install(BcPackTables.Load(path));

            return true;
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or InvalidDataException)
        {
            error.WriteLine($"{Option} {path}: {exception.Message}");

            return false;
        }
    }

    /// <summary>One line on what decodes, for reports.</summary>
    public static string Describe()
    {
        if (BcPackPageDecoder.Tables is not null)
        {
            return "BCPack tables loaded: every codec decodes exactly";
        }

        return $"no BCPack tables ({Option} <file> or {Variable}): {Fallback}";
    }
}
