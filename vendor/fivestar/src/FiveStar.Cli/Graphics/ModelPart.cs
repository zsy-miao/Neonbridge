// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using FiveStar.Formats.Models;
using FiveStar.Graphics.Rendering;

namespace FiveStar.Cli.Graphics;

/// <summary>Geometries placed in the model by a transform (row vectors) and drawn with one texture.</summary>
public sealed record ModelPart(string Name, IReadOnlyList<MeshGeometry> Geometries, Matrix4x4 Transform, TextureData Texture);
