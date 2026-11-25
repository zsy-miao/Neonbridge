// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// Semantic names, their kinds and how each signature point treats them (the subset of DxilSigPoint.inl the
/// translator's stages use), and the interpolation modes dxc gives elements without a modifier.
/// </summary>
public static class Semantics
{
    /// <summary>DXIL interpolation modes.</summary>
    public const byte Constant = 1;
    public const byte Linear = 2;
    public const byte LinearCentroid = 3;
    public const byte LinearNoPerspective = 4;
    public const byte LinearNoPerspectiveCentroid = 5;
    public const byte LinearSample = 6;
    public const byte LinearNoPerspectiveSample = 7;

    private const string SystemPrefix = "SV_";

    private static readonly Dictionary<string, (string Name, SemanticKind Kind)> SystemValues = new[]
    {
        ("SV_VertexID", SemanticKind.VertexId), ("SV_InstanceID", SemanticKind.InstanceId), ("SV_Position", SemanticKind.Position),
        ("SV_RenderTargetArrayIndex", SemanticKind.RenderTargetArrayIndex), ("SV_ViewportArrayIndex", SemanticKind.ViewportArrayIndex),
        ("SV_ClipDistance", SemanticKind.ClipDistance), ("SV_CullDistance", SemanticKind.CullDistance), ("SV_PrimitiveID", SemanticKind.PrimitiveId),
        ("SV_GSInstanceID", SemanticKind.GsInstanceId), ("SV_SampleIndex", SemanticKind.SampleIndex), ("SV_IsFrontFace", SemanticKind.IsFrontFace),
        ("SV_Coverage", SemanticKind.Coverage), ("SV_InnerCoverage", SemanticKind.InnerCoverage), ("SV_Target", SemanticKind.Target),
        ("SV_Depth", SemanticKind.Depth), ("SV_DepthLessEqual", SemanticKind.DepthLessEqual), ("SV_DepthGreaterEqual", SemanticKind.DepthGreaterEqual),
        ("SV_StencilRef", SemanticKind.StencilRef), ("SV_DispatchThreadID", SemanticKind.DispatchThreadId), ("SV_GroupID", SemanticKind.GroupId),
        ("SV_GroupIndex", SemanticKind.GroupIndex), ("SV_GroupThreadID", SemanticKind.GroupThreadId), ("SV_ViewID", SemanticKind.ViewId),
        ("SV_Barycentrics", SemanticKind.Barycentrics), ("SV_ShadingRate", SemanticKind.ShadingRate), ("SV_CullPrimitive", SemanticKind.CullPrimitive),
    }.ToDictionary(entry => entry.Item1, entry => (entry.Item1, entry.Item2), StringComparer.OrdinalIgnoreCase);

    private static readonly Dictionary<(SemanticKind Kind, SignaturePoint Point), SemanticInterpretation> Interpretations = Table();

    /// <summary>Splits a semantic into its name and index (<c>TEXCOORD11</c>: TEXCOORD, 11) and finds its kind.</summary>
    public static (string Name, int Index, SemanticKind Kind) Parse(string semantic)
    {
        var digits = semantic.Length;

        while (digits > 0 && char.IsAsciiDigit(semantic[digits - 1]))
        {
            digits--;
        }

        var name = semantic[..digits];
        var index = 0;

        if (digits < semantic.Length)
        {
            index = int.Parse(semantic.AsSpan(digits), System.Globalization.CultureInfo.InvariantCulture);
        }

        if (!name.StartsWith(SystemPrefix, StringComparison.OrdinalIgnoreCase))
        {
            return (name, index, SemanticKind.Arbitrary);
        }

        if (!SystemValues.TryGetValue(name, out var system))
        {
            throw new DxilBackendException($"semantic {semantic} is not supported");
        }

        return (system.Name, index, system.Kind);
    }

    /// <summary>How <paramref name="point"/> treats <paramref name="kind"/>; a semantic it does not allow throws.</summary>
    public static SemanticInterpretation Interpret(SemanticKind kind, SignaturePoint point)
    {
        if (!Interpretations.TryGetValue((kind, point), out var interpretation))
        {
            throw new DxilBackendException($"semantic kind {kind} is not allowed at {point}");
        }

        return interpretation;
    }

    /// <summary>Whether elements at <paramref name="point"/> carry an interpolation mode.</summary>
    public static bool HasInterpolation(SignaturePoint point)
    {
        return point is not (SignaturePoint.VertexInput or SignaturePoint.PixelOutput or SignaturePoint.GeometryInput);
    }

    /// <summary>
    /// The interpolation mode of an element: from its modifiers, else linear for floats and constant for the rest (and
    /// for per-primitive outputs); the position is never perspective-corrected.
    /// </summary>
    public static byte Interpolation(SignaturePoint point, SemanticKind kind, bool isFloat, IReadOnlySet<string> modifiers)
    {
        if (!HasInterpolation(point))
        {
            return 0;
        }

        if (modifiers.Contains("nointerpolation") || point == SignaturePoint.MeshPrimitiveOutput || (!isFloat && !modifiers.Contains("linear")))
        {
            return Constant;
        }

        var perspective = !modifiers.Contains("noperspective") && kind != SemanticKind.Position;

        if (modifiers.Contains("sample"))
        {
            return Choose(perspective, LinearSample, LinearNoPerspectiveSample);
        }

        if (modifiers.Contains("centroid"))
        {
            return Choose(perspective, LinearCentroid, LinearNoPerspectiveCentroid);
        }

        return Choose(perspective, Linear, LinearNoPerspective);
    }

    private static byte Choose(bool perspective, byte withPerspective, byte without)
    {
        if (perspective)
        {
            return withPerspective;
        }

        return without;
    }

    private static Dictionary<(SemanticKind Kind, SignaturePoint Point), SemanticInterpretation> Table()
    {
        var table = new Dictionary<(SemanticKind Kind, SignaturePoint Point), SemanticInterpretation>();
        SignaturePoint[] vertexLike = [SignaturePoint.VertexOutput, SignaturePoint.PixelInput, SignaturePoint.GeometryVertexInput, SignaturePoint.GeometryOutput];

        foreach (var point in Enum.GetValues<SignaturePoint>().Where(point => point != SignaturePoint.PixelOutput))
        {
            table[(SemanticKind.Arbitrary, point)] = SemanticInterpretation.Arbitrary;
        }

        Add(table, [SemanticKind.VertexId, SemanticKind.InstanceId], [SignaturePoint.VertexInput], SemanticInterpretation.SystemValue);
        Add(table, [SemanticKind.Position], [.. vertexLike, SignaturePoint.MeshVertexOutput], SemanticInterpretation.SystemValue);
        table[(SemanticKind.Position, SignaturePoint.VertexInput)] = SemanticInterpretation.Arbitrary;
        SemanticKind[] layers = [SemanticKind.RenderTargetArrayIndex, SemanticKind.ViewportArrayIndex, SemanticKind.ShadingRate];
        Add(table, layers, [.. vertexLike, SignaturePoint.MeshPrimitiveOutput], SemanticInterpretation.SystemValue);
        Add(table, [SemanticKind.ClipDistance, SemanticKind.CullDistance], [.. vertexLike, SignaturePoint.MeshVertexOutput], SemanticInterpretation.ClipCull);

        Add(table, [SemanticKind.PrimitiveId], [SignaturePoint.GeometryOutput, SignaturePoint.MeshPrimitiveOutput], SemanticInterpretation.SystemValue);
        Add(table, [SemanticKind.PrimitiveId, SemanticKind.IsFrontFace], [SignaturePoint.PixelInput], SemanticInterpretation.SystemGenerated);
        table[(SemanticKind.IsFrontFace, SignaturePoint.GeometryOutput)] = SemanticInterpretation.SystemGenerated;

        table[(SemanticKind.SampleIndex, SignaturePoint.PixelInput)] = SemanticInterpretation.Shadow;
        table[(SemanticKind.PrimitiveId, SignaturePoint.GeometryInput)] = SemanticInterpretation.Shadow;
        table[(SemanticKind.GsInstanceId, SignaturePoint.GeometryInput)] = SemanticInterpretation.NotInSignature;
        Add(table, [SemanticKind.Coverage, SemanticKind.InnerCoverage], [SignaturePoint.PixelInput], SemanticInterpretation.NotInSignature);
        table[(SemanticKind.Barycentrics, SignaturePoint.PixelInput)] = SemanticInterpretation.NotPacked;

        table[(SemanticKind.Target, SignaturePoint.PixelOutput)] = SemanticInterpretation.Target;
        SemanticKind[] pixelOutputs = [SemanticKind.Depth, SemanticKind.DepthLessEqual, SemanticKind.DepthGreaterEqual, SemanticKind.StencilRef, SemanticKind.Coverage];
        Add(table, pixelOutputs, [SignaturePoint.PixelOutput], SemanticInterpretation.NotPacked);
        table[(SemanticKind.CullPrimitive, SignaturePoint.MeshPrimitiveOutput)] = SemanticInterpretation.NotPacked;

        return table;
    }

    private static void Add(
        Dictionary<(SemanticKind Kind, SignaturePoint Point), SemanticInterpretation> table, SemanticKind[] kinds, SignaturePoint[] points, SemanticInterpretation interpretation)
    {
        foreach (var kind in kinds)
        {
            foreach (var point in points)
            {
                table[(kind, point)] = interpretation;
            }
        }
    }
}
