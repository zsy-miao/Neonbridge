// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json.Serialization;

namespace FiveStar.Graphics.Programs;

/// <summary>The JSON form of <see cref="ShaderManifest"/>: camelCase, indented, absent values left out.</summary>
[JsonSourceGenerationOptions(
    PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase,
    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    WriteIndented = true)]
[JsonSerializable(typeof(ShaderManifest))]
internal sealed partial class ManifestJsonContext : JsonSerializerContext;
