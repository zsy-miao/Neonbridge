// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Formats.Pso;

/// <summary>A parsed PSO: its schemas, enums and data blocks over the original bytes.</summary>
/// <param name="Layout">File or PRD0 resource.</param>
/// <param name="Bytes">The whole file or resource segment.</param>
/// <param name="Chunks">The file's chunks (empty for a resource).</param>
/// <param name="Structs">Struct schemas by name hash, in table order.</param>
/// <param name="Enums">Enum values by enum hash, in table order.</param>
/// <param name="Blocks">Data blocks; block ids are 1-based.</param>
/// <param name="Root">The root block id, or null when the file names none.</param>
/// <param name="DataEnd">File only: where the last data block ends.</param>
/// <param name="PsinEnd">File only: where the <c>PSIN</c> chunk ends.</param>
public sealed record PsoDocument(
    PsoLayout Layout,
    byte[] Bytes,
    IReadOnlyList<PsoChunk> Chunks,
    OrderedMap<uint, PsoStruct> Structs,
    OrderedMap<uint, IReadOnlyList<PsoEnumValue>> Enums,
    IReadOnlyList<PsoBlock> Blocks,
    long? Root,
    long DataEnd,
    long PsinEnd)
{
    /// <summary>
    /// The block with a 1-based id. Id 0 and negative ids count from the end (id 0 = the last block), as a
    /// zero-based index of -1 would; ids past the end throw.
    /// </summary>
    public PsoBlock BlockAt(long id)
    {
        var index = id - 1;

        if (index < 0)
        {
            index += Blocks.Count;
        }

        if (index < 0 || index >= Blocks.Count)
        {
            throw new PsoException("list index out of range", false);
        }

        return Blocks[(int)index];
    }

    /// <summary>The struct schema with a name hash; throws when the schema has none.</summary>
    public PsoStruct StructNamed(uint name)
    {
        if (!Structs.TryGetValue(name, out var schema))
        {
            throw new PsoException(name.ToString(System.Globalization.CultureInfo.InvariantCulture), false);
        }

        return schema;
    }
}
