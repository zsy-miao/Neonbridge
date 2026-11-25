// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Renames the game's vertex elements to what a translated vertex shader reads: fetched attribute n is <c>ATTR</c>
/// index n, and attribute n is the n-th non-system input of the shader's ISG1 signature. Elements are matched to the
/// signature by semantic name and index, in stream and cache layouts alike (61 cache layouts list them in another
/// order). Elements the shader does not read get unique placeholder names.
/// </summary>
internal static class InputLayoutMapping
{
    public const string AttributeSemantic = "ATTR";
    public const string UnusedSemantic = "UNUSED";

    private const string InputSignature = "ISG1";

    /// <summary>
    /// The layout for a vertex shader with the non-system <paramref name="inputs"/> (<see cref="Inputs"/>); without
    /// them the layout is taken to be in attribute order, element n becoming attribute n.
    /// </summary>
    public static IReadOnlyList<VertexElement> Map(IReadOnlyList<VertexElement> layout, IReadOnlyList<SignatureElement>? inputs)
    {
        if (inputs is null)
        {
            return layout.Select((element, index) => element with { Semantic = AttributeSemantic, SemanticIndex = index }).ToList();
        }

        var mapped = new List<VertexElement>(layout.Count);
        var unused = 0;

        foreach (var element in layout)
        {
            var attribute = Attribute(inputs, element);

            if (attribute < 0)
            {
                mapped.Add(element with { Semantic = UnusedSemantic, SemanticIndex = unused++ });

                continue;
            }

            mapped.Add(element with { Semantic = AttributeSemantic, SemanticIndex = attribute });
        }

        return mapped;
    }

    /// <summary>The non-system inputs of a DXBC container's ISG1 signature, in attribute order; empty without one.</summary>
    public static IReadOnlyList<SignatureElement> Inputs(byte[] container)
    {
        var signature = DxbcContainer.Parse(container).FindFirst(InputSignature);

        if (signature is null)
        {
            return [];
        }

        return SignatureParser.Parse(signature.Data.Span).Where(input => input.SystemValue.Length == 0).ToList();
    }

    private static int Attribute(IReadOnlyList<SignatureElement> inputs, VertexElement element)
    {
        for (var index = 0; index < inputs.Count; index++)
        {
            if (string.Equals(inputs[index].Name, element.Semantic, StringComparison.OrdinalIgnoreCase) && inputs[index].SemanticIndex == element.SemanticIndex)
            {
                return index;
            }
        }

        return -1;
    }
}
