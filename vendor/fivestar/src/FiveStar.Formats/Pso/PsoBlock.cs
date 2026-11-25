// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>A data block: the struct or primitive type it holds, where it starts and its size.</summary>
/// <param name="Type">Struct name hash, synthesized struct id or primitive type code.</param>
/// <param name="Offset">Byte offset in the buffer; null for a block in a resource's physical segment.</param>
/// <param name="Size">Size in bytes.</param>
public readonly record struct PsoBlock(uint Type, long? Offset, uint Size);
