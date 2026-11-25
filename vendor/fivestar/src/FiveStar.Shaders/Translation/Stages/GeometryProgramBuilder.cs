// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Legacy geometry shaders with on-chip GSVS. The main program reads its input vertices from the ESGS ring in LDS,
/// writes output vertices to the GSVS ring in LDS and signals GS_EMIT/GS_CUT; the driver's VS copy shader (the FSSM's
/// second program block) reads one GSVS vertex and exports it. In HLSL one GS invocation is one lane with a private
/// static LDS array: the input vertices are stored at GS_IN_BASE (ESGS layout: register r component c at dword r*4+c),
/// and GS_EMIT calls the translated copy shader on the current vertex record and appends the result to the stream.
/// </summary>
internal static partial class GeometryProgramBuilder
{
    private const string PreludeInclude = "#include \"rdna_prelude.hlsli\"\n";
    private const string CopyEntry = "Out main(uint in_vid : SV_VertexID, uint in_iid : SV_InstanceID) {";
    private const string CopyFunction = "Out gs_copy(uint in_vid, uint in_iid) {";
    private const string Components = "xyzw";
    private const int Vec4 = 4;

    // Input vertices are placed after the output records, from dword 256, on a 64-dword boundary.
    private const int MinimumInputBase = 256;
    private const int InputBaseAlignment = 64;

    // PSV0 geometry input primitive (D3D_PRIMITIVE) -> HLSL primitive type and vertex count.
    private static readonly IReadOnlyDictionary<int, (string Primitive, int Vertices)> InputPrimitives =
        new Dictionary<int, (string, int)>
        {
            [1] = ("point", 1),
            [2] = ("line", 2),
            [3] = ("triangle", 3),
            [6] = ("lineadj", 4),
            [7] = ("triangleadj", 6),
        };

    // PSV0 geometry output topology (D3D_PRIMITIVE_TOPOLOGY) -> HLSL stream type.
    private static readonly IReadOnlyDictionary<int, string> OutputStreams = new Dictionary<int, string>
    {
        [1] = "PointStream",
        [2] = "LineStream",
        [3] = "LineStream",
        [4] = "TriangleStream",
        [5] = "TriangleStream",
    };

    public static string Build(ShaderProgram program, TranslationOptions options)
    {
        var psv = program.PipelineState;
        var recordDwords = GsVertexLayout.VertexDwords(program.Instructions);
        var inputCode = psv?.GeometryInputPrimitive ?? 0;
        var outputCode = psv?.GeometryOutputTopology ?? 0;

        if (!InputPrimitives.TryGetValue(inputCode, out var input))
        {
            throw new UnsupportedProgramException($"GS input primitive {Describe(psv?.GeometryInputPrimitive)}");
        }

        if (!OutputStreams.TryGetValue(outputCode, out var stream))
        {
            throw new UnsupportedProgramException($"GS output topology {Describe(psv?.GeometryOutputTopology)}");
        }

        var (primitive, inputVertices) = input;
        var maxVertices = psv?.MaxVertexCount ?? 0;

        if (maxVertices == 0)
        {
            throw new UnsupportedProgramException("GS max vertex count unknown");
        }

        var copy = CopyShader(program, options);
        var main = ProgramTranslator.Translate(program, program.Instructions, ShaderStage.Geometry, copyShader: false, options);
        main = ExportStructPattern().Replace(main.Replace(PreludeInclude, string.Empty, StringComparison.Ordinal), string.Empty);

        var inputs = program.Signatures.Inputs;
        var registers = inputs.Count == 0 ? 0 : inputs.Max(element => element.Register) + 1;
        var inputStride = Math.Max(Vec4, registers * Vec4);
        var inputBase = Math.Max(MinimumInputBase, ((recordDwords * maxVertices) + InputBaseAlignment - 1) / InputBaseAlignment * InputBaseAlignment);
        var layout = new GeometryLayout(recordDwords, inputStride, inputBase, maxVertices, inputVertices);
        var (inputStruct, fill) = InputStruct(inputs, inputVertices);

        var entry = $"[maxvertexcount({maxVertices})]\nvoid main({primitive} GSIn gin[{inputVertices}], uint in_prim : SV_PrimitiveID, "
            + $"inout {stream}<Out> gs_os) {{\n" + string.Join("\n", fill);
        var placeholder = ProgramEmitter.GeometryEntryPlaceholder + " {";

        if (!main.Contains(placeholder, StringComparison.Ordinal))
        {
            throw new UnsupportedProgramException("GS entry placeholder missing");
        }

        main = main.Replace(placeholder, entry, StringComparison.Ordinal);

        return Head(layout) + copy + "\n" + string.Join("\n", inputStruct) + "\n" + main;
    }

    /// <summary>The copy shader as a function <c>gs_copy</c> returning the Out struct.</summary>
    private static string CopyShader(ShaderProgram program, TranslationOptions options)
    {
        var copyProgram = program with
        {
            Code = null,
            Signatures = new ShaderSignatures([], program.Signatures.Outputs),
            Config = new ShaderConfig(),
            PipelineState = null,
        };
        var instructions = program.CopyShader ?? throw new UnsupportedProgramException("GS copy shader missing");
        var text = ProgramTranslator.Translate(copyProgram, instructions, ShaderStage.Vertex, copyShader: true, options);

        if (!text.Contains(CopyEntry, StringComparison.Ordinal))
        {
            throw new UnsupportedProgramException("GS copy shader entry not recognised");
        }

        return text.Replace(CopyEntry, CopyFunction, StringComparison.Ordinal).Replace(PreludeInclude, string.Empty, StringComparison.Ordinal);
    }

    private static (List<string> Struct, List<string> Fill) InputStruct(IReadOnlyList<SignatureElement> inputs, int inputVertices)
    {
        var lines = new List<string> { "struct GSIn {" };
        var fill = new List<string>();

        foreach (var element in inputs)
        {
            var mask = element.Mask;
            var member = $"r{element.Register}_{mask}";
            lines.Add($"  {SignatureLayout.TypeName(element)}{SignatureLayout.Width(mask)} {member} : {element.Name}{element.SemanticIndex};");

            for (var vertex = 0; vertex < inputVertices; vertex++)
            {
                for (var component = 0; component < mask.Length; component++)
                {
                    var source = mask.Length > 1 ? $"gin[{vertex}].{member}.{Components[component]}" : $"gin[{vertex}].{member}";
                    var dword = (element.Register * Vec4) + Components.IndexOf(mask[component], StringComparison.Ordinal);
                    fill.Add($"  g_lds[GS_IN_BASE + {vertex}u * GS_IN_STRIDE + {dword}u] = asuint({source});");
                }
            }
        }

        lines.Add("};");

        return (lines, fill);
    }

    private static string Head(GeometryLayout layout)
    {
        var head = new StringBuilder();
        head.Append(PreludeInclude);
        head.Append($"// legacy GS (on-chip GSVS): vertex record {layout.RecordDwords} dwords, input vertex stride {layout.InputStride} dwords, ");
        head.Append($"{layout.MaxVertices} output vertices max\n");

        head.Append("#define RING_TAG 0xffu\n");
        head.Append($"#define GS_VTX_DW {layout.RecordDwords}u\n");
        head.Append($"#define GS_IN_BASE {layout.InputBase}u\n");
        head.Append($"#define GS_IN_STRIDE {layout.InputStride}u\n");
        head.Append($"static uint g_lds[{layout.InputBase + (layout.InputVertices * layout.InputStride)}];\n");

        return head.ToString();
    }

    private static string Describe(int? value)
    {
        if (value is null)
        {
            return "None";
        }

        return value.Value.ToString(System.Globalization.CultureInfo.InvariantCulture);
    }

    [GeneratedRegex(@"^struct (Exp|Out) \{\n.*?\n\};\n", RegexOptions.Singleline | RegexOptions.Multiline)]
    private static partial Regex ExportStructPattern();
}
