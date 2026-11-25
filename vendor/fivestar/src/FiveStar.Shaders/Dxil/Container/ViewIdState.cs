// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// The view-ID state of a graphics shader (<c>dx.viewIdState</c>, and the dependency tables of PSV0): the signature
/// scalar counts and which outputs each input scalar feeds. Like VIctory, the backend declares that every output
/// depends on every input instead of tracing the dependencies; drivers only use the tables to drop work.
/// </summary>
public static class ViewIdState
{
    private const int Components = 4;
    private const int BitsPerWord = 32;
    private const int Streams = 4;

    /// <summary>The scalars up to the last allocated component of <paramref name="elements"/>.</summary>
    public static int Scalars(IReadOnlyList<DxilSignatureElement> elements)
    {
        return elements.Where(element => element.IsAllocated)
            .Select(element => ((element.StartRow + element.Rows - 1) * Components) + element.StartColumn + element.Columns)
            .DefaultIfEmpty(0).Max();
    }

    /// <summary>The registers <paramref name="elements"/> use.</summary>
    public static int Vectors(IReadOnlyList<DxilSignatureElement> elements)
    {
        return elements.Where(element => element.IsAllocated).Select(element => element.StartRow + element.Rows).DefaultIfEmpty(0).Max();
    }

    /// <summary>
    /// The serialized state (DxilViewIdState::Serialize): the input scalars, then per output stream its scalars and its
    /// table (four streams for a geometry shader, only the first used), then a mesh shader's primitive scalars and table.
    /// </summary>
    public static uint[] Serialize(ShaderDescription description)
    {
        var inputs = Scalars(description.Inputs);
        var words = new List<uint> { (uint)inputs };
        Table(words, inputs, Scalars(description.Outputs));

        if (description.Kind == DxilShaderKind.Geometry)
        {
            for (var stream = 1; stream < Streams; stream++)
            {
                Table(words, inputs, 0);
            }
        }

        if (description.Kind == DxilShaderKind.Mesh)
        {
            Table(words, inputs, Scalars(description.Primitives));
        }

        return [.. words];
    }

    /// <summary>An output count and the words of every input's dependencies on those outputs.</summary>
    private static void Table(List<uint> words, int inputs, int outputs)
    {
        words.Add((uint)outputs);
        var stride = (outputs + BitsPerWord - 1) / BitsPerWord;

        for (var input = 0; input < inputs; input++)
        {
            for (var word = 0; word < stride; word++)
            {
                words.Add(Dependencies(word, outputs));
            }
        }
    }

    /// <summary>The bits of dependency word <paramref name="word"/>: every output below <paramref name="outputs"/>.</summary>
    public static uint Dependencies(int word, int outputs)
    {
        var bits = Math.Clamp(outputs - (word * BitsPerWord), 0, BitsPerWord);

        if (bits == BitsPerWord)
        {
            return uint.MaxValue;
        }

        return (1u << bits) - 1;
    }
}
