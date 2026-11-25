// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// results.json: a JSON array with one object per program, <c>{"name": ..., "stage": ..., "status": ..., "reason": ...}</c>,
/// stage and reason null when unknown.
/// </summary>
public static class ResultsFile
{
    public static IReadOnlyList<ProgramResult> Read(string path)
    {
        using var document = JsonDocument.Parse(File.ReadAllText(path));
        var results = new List<ProgramResult>();

        foreach (var element in document.RootElement.EnumerateArray())
        {
            results.Add(new ProgramResult(
                element.GetProperty("name").GetString() ?? string.Empty,
                OptionalString(element, "stage"),
                element.GetProperty("status").GetString() ?? string.Empty,
                OptionalString(element, "reason")));
        }

        return results;
    }

    public static void Write(string path, IEnumerable<ProgramResult> results)
    {
        var text = new StringBuilder("[");
        var first = true;

        foreach (var result in results)
        {
            if (!first)
            {
                text.Append(", ");
            }

            first = false;
            text.Append($"{{\"name\": {Value(result.Name)}, \"stage\": {Value(result.Stage)}, ");
            text.Append($"\"status\": {Value(result.Status)}, \"reason\": {Value(result.Reason)}}}");
        }

        text.Append(']');
        File.WriteAllText(path, text.ToString());
    }

    private static string? OptionalString(JsonElement element, string name)
    {
        if (!element.TryGetProperty(name, out var value) || value.ValueKind != JsonValueKind.String)
        {
            return null;
        }

        return value.GetString();
    }

    private static string Value(string? text)
    {
        if (text is null)
        {
            return "null";
        }

        // The escaping JsonSerializer applies to a string, without its reflection-based serializer (trim and AOT safe).
        return $"\"{JsonEncodedText.Encode(text, JavaScriptEncoder.UnsafeRelaxedJsonEscaping)}\"";
    }
}
