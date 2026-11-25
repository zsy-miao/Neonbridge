// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;
using System.Text.Json.Serialization;

namespace FiveStar.Cli.Progress;

/// <summary>Reads and writes <c>status.json</c> and the manual file (camelCase JSON, nulls left out).</summary>
internal static class StatusFile
{
    public static ProgressStatus Read(string path)
    {
        using var stream = File.OpenRead(path);
        var status = JsonSerializer.Deserialize<ProgressStatus>(stream, Options());

        if (status is null)
        {
            throw new InvalidDataException(path + ": empty status file");
        }

        return status;
    }

    public static void Write(string path, ProgressStatus status)
    {
        File.WriteAllText(path, JsonSerializer.Serialize(status, Options()) + Environment.NewLine);
    }

    /// <summary>Reads a JSON file as a list of <typeparamref name="T"/>.</summary>
    public static List<T> ReadList<T>(string path)
    {
        using var stream = File.OpenRead(path);
        var list = JsonSerializer.Deserialize<List<T>>(stream, Options());

        return list ?? [];
    }

    private static JsonSerializerOptions Options()
    {
        return new JsonSerializerOptions(JsonSerializerDefaults.Web)
        {
            WriteIndented = true,
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
            ReadCommentHandling = JsonCommentHandling.Skip,
            AllowTrailingCommas = true,
        };
    }
}
