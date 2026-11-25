// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>The short name and fixed size of a type code.</summary>
/// <param name="Code">The type code.</param>
/// <param name="Name">Short name used in schema listings.</param>
/// <param name="Size">Size in bytes; null when it depends on the schema (inline structs and fixed arrays).</param>
public sealed record PsoTypeInfo(PsoTypeCode Code, string Name, int? Size);
