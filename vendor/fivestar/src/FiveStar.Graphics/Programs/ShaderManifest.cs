// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;

namespace FiveStar.Graphics.Programs;

/// <summary>
/// <c>variants.json</c> next to the translated programs: every compiled program with its forms and wave facts, so a
/// runtime picks the DXIL per adapter (<see cref="VariantSelector"/>) without reading HLSL.
/// </summary>
public sealed record ShaderManifest(IReadOnlyList<ProgramVariants> Programs)
{
    public const string FileName = "variants.json";

    public static ShaderManifest Read(string path)
    {
        using var stream = File.OpenRead(path);

        return JsonSerializer.Deserialize(stream, ManifestJsonContext.Default.ShaderManifest)
            ?? throw new InvalidDataException($"{path} is not a shader manifest");
    }

    public void Write(string path)
    {
        var temporary = path + ".tmp";

        using (var stream = File.Create(temporary))
        {
            JsonSerializer.Serialize(stream, this, ManifestJsonContext.Default.ShaderManifest);
        }

        File.Move(temporary, path, overwrite: true);
    }
}
