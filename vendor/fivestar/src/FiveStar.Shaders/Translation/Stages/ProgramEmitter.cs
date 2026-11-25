// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using FiveStar.Shaders.Translation.Waves;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Assembles the HLSL program from the translated blocks: includes, input/output structs, resource declarations,
/// the entry point with its register variables, the prologue and the block dispatch, then the output mapping.
/// </summary>
internal sealed partial class ProgramEmitter
{
    /// <summary>The entry line of a geometry program, filled in by the geometry builder.</summary>
    public const string GeometryEntryPlaceholder = "/*GS_MAIN*/";

    private const string OutStruct = "struct Out {";
    private const string SuperstepHeader = "rdna_superstep.hlsli";
    private const int DefaultComputeThreads = 64;
    private const int BytesPerDword = 4;
    private const int CodeLoadLine = 2;

    // Compute programs without an LDS size in the header get the whole 64 KB.
    private const int DefaultLdsBytes = 65536;

    private readonly TranslationState _state;
    private readonly OutputWriter _outputs;
    private readonly MeshDeclarations _mesh;
    private readonly BodyWriter _body;

    public ProgramEmitter(TranslationState state, OutputWriter outputs, MeshDeclarations mesh, BodyWriter body)
    {
        _state = state;
        _outputs = outputs;
        _mesh = mesh;
        _body = body;
    }

    public string Emit()
    {
        var lines = Header();

        if (_state.Stage == ShaderStage.Pixel)
        {
            new PixelInputWriter(_state).Write(lines);
        }

        _outputs.Write(lines);
        InsertFetchInputs(lines);
        var entry = EntrySignature(lines);
        InsertResources(lines);

        lines.Add(entry + " {");
        _body.Write(lines);

        return Finish(string.Join("\n", lines) + "\n");
    }

    private List<string> Header()
    {
        const string Include = "#include \"rdna_prelude.hlsli\"";

        var lines = new List<string> { Include };

        if (_state.EmulateWaves)
        {
            // One entry, so line positions after the include stay as they are.
            lines[0] = string.Join("\n", WaveDefines()) + "\n" + Include;
        }

        if (_state.Supersteps)
        {
            // Written into the program, not included: the headers every program includes stay as they are.
            lines[0] += "\n" + RuntimeIncludes.Read(SuperstepHeader).TrimEnd('\n');
        }

        if (_state.UsesBvh)
        {
            lines.Add("#include \"rdna_bvh.hlsli\"");
        }

        lines.AddRange(_state.StaticSamplers.Select(sampler => sampler.Line));

        lines.Add(string.Empty);

        return lines;
    }

    /// <summary>The prelude switches for wave emulation and the program's wave floor (see WaveOps.FloorLine).</summary>
    private List<string> WaveDefines()
    {
        var wave = _state.WaveSize;
        var floor = Math.Max(_state.LaneReach, WaveOps.NarrowestHardwareWave);
        var lines = new List<string> { $"#define VW_WAVE {wave}", "#define VW_EMULATE 1" };

        if (_state.Supersteps)
        {
            // Every wave operation exchanges through groupshared memory; quads stay native.
            lines.AddRange(["#define VW_GROUP_LANES 1", $"#define VW_GROUP_THREADS {_state.GroupThreads}u"]);

            if (_state.SuperstepValues)
            {
                lines.Add($"#define VW_SS_VALUES {SuperstepSplitter.FirstLaneValues}");
            }

            lines.Insert(0, WaveOps.FloorLine(wave, WaveOps.NarrowestHardwareWave));

            return lines;
        }

        if (_state.Stage is ShaderStage.Pixel or ShaderStage.Vertex && floor > WaveOps.NarrowestHardwareWave)
        {
            // Lanes a narrower wave lacks are console lanes without a pixel or vertex; reads of them may hold.
            var analysis = new MissingLaneAnalysis(_state.Blocks, _state.Prologue, wave);
            floor = analysis.Floor(floor);
            analysis.Rewrite(_state.Blocks);

            if (analysis.Reason is not null)
            {
                lines.Insert(0, WaveOps.NarrowerLine(analysis.Reason));
            }
        }

        if (_state.Stage == ShaderStage.Mesh)
        {
            lines.Add("#define VW_GROUP_LANES 1");
            floor = MeshFloor(lines, wave, floor);
        }

        lines.Insert(0, WaveOps.FloorLine(wave, floor));

        return lines;
    }

    /// <summary>
    /// A mesh program's floor: the other lanes of its wave are live threads of other hardware waves (see WindowAnalysis),
    /// so it needs its whole wave unless the analysis shows otherwise, or fast waves take it approximately.
    /// </summary>
    private int MeshFloor(List<string> lines, int wave, int floor)
    {
        var reason = new WindowAnalysis(_state.Blocks, _state.Prologue, _state.MeshThreads).Check();

        if (reason is null)
        {
            return floor;
        }

        if (_state.FastWaves)
        {
            var fast = new WindowAnalysis(_state.Blocks, _state.Prologue, _state.MeshThreads) { FastWaves = true };

            if (fast.Check() is null)
            {
                lines.Insert(0, WaveOps.FastLine(reason));
                EnterAlways(fast.EnteredAlways);

                return floor;
            }
        }

        lines.Insert(0, WaveOps.NarrowerLine(reason));

        return wave;
    }

    /// <summary>Fast waves: the skips every window takes become jumps into the path they skip (WindowAnalysis.EntersAlways).</summary>
    private void EnterAlways(IReadOnlyDictionary<int, int> paths)
    {
        for (var index = 0; index < _state.Blocks.Count; index++)
        {
            var block = _state.Blocks[index];

            if (!paths.TryGetValue(block.Address, out var path))
            {
                continue;
            }

            var lines = new List<string>(block.Lines);
            var branch = lines.FindLastIndex(line => BranchPattern().IsMatch(line));

            if (branch >= 0)
            {
                lines[branch] = $"  pc = {Hlsl.Hex(path)};";
            }

            _state.Blocks[index] = new CodeBlock(block.Address, lines, new BlockExit(BlockExitKind.Jump, path, null, null));
        }
    }

    /// <summary>Vertex attributes the driver's fetch shader would load (table @0x138); the pipeline binds ATTR&lt;n&gt;.</summary>
    private void InsertFetchInputs(List<string> lines)
    {
        if (_state.FetchInputs.Count == 0)
        {
            return;
        }

        var members = _state.FetchInputs.OrderBy(input => input.Attribute).ThenBy(input => input.Components)
            .Select(input => $"  float4 a{input.Attribute} : ATTR{input.Attribute};");
        lines.Insert(lines.IndexOf(OutStruct), "struct FetchIn {\n" + string.Join("\n", members) + "\n};");
    }

    private string EntrySignature(List<string> lines)
    {
        switch (_state.Stage)
        {
            case ShaderStage.Vertex:
                return VertexEntry();
            case ShaderStage.Mesh:
                return MeshEntry(lines);
            case ShaderStage.Geometry:
                return GeometryEntryPlaceholder;
            case ShaderStage.Compute:
                return ComputeEntry();
            default:
                return PixelEntry();
        }
    }

    private string VertexEntry()
    {
        var fetch = _state.FetchInputs.Count > 0 ? ", FetchIn fin" : string.Empty;

        return $"Out main(uint in_vid : SV_VertexID, uint in_iid : SV_InstanceID{fetch})";
    }

    private string MeshEntry(List<string> lines)
    {
        var declarations = _state.MeshYield ? MeshDeclarations.DirectDeclarations() : _mesh.StagingDeclarations(_outputs.OutputRegisters());
        lines.Insert(lines.IndexOf(OutStruct), declarations);

        if (!_state.Outputs.Contains("pos0"))
        {
            throw new UnsupportedProgramException("mesh shader without position export");
        }

        if (_state.MeshYield)
        {
            _mesh.ExpandDirectExports();
        }

        return _mesh.EntrySignature();
    }

    private string ComputeEntry()
    {
        var threads = _state.Config.Compute?.NumThreads;
        var (x, y, z) = (DefaultComputeThreads, 1, 1);

        if (threads is not null)
        {
            (x, y, z) = (threads.Value.X, threads.Value.Y, threads.Value.Z);
        }

        return $"[numthreads({x}, {y}, {z})]\nvoid main(uint3 in_gid : SV_GroupID, uint3 in_gtid : SV_GroupThreadID, uint in_gi : SV_GroupIndex)";
    }

    private string PixelEntry()
    {
        var used = _state.Inputs.Values.ToHashSet();
        var parameters = new List<string>();

        if (_state.Attributes.Count > 0)
        {
            parameters.Add("PSIn pin");
        }

        if (used.Overlaps(["pos_x", "pos_y", "pos_z", "pos_w", "pos"]))
        {
            parameters.Add("float4 in_pos : SV_Position");
        }

        (string Input, string Parameter)[] system =
        [
            ("front_face", "bool in_front : SV_IsFrontFace"),
            ("ancillary", "uint in_sample : SV_SampleIndex"),
            ("sample_coverage", "uint in_coverage : SV_Coverage"),
            ("bary", "float3 in_bary : SV_Barycentrics"),
        ];
        parameters.AddRange(system.Where(entry => used.Contains(entry.Input)).Select(entry => entry.Parameter));

        return $"Out main({string.Join(", ", parameters)})";
    }

    private void InsertResources(List<string> lines)
    {
        if (_state.UsesLds && _state.Stage != ShaderStage.Geometry && !_state.IsCopyShader)
        {
            // Geometry programs use a private static array, declared by the geometry builder.
            var bytes = _state.Config.LdsBytes ?? 0;

            if (bytes == 0)
            {
                bytes = DefaultLdsBytes;
            }

            var dwords = Math.Max(1, bytes / BytesPerDword);
            lines.Insert(1, $"groupshared uint g_lds[{dwords}];");
        }

        if (_state.UsesCode)
        {
            var code = _state.Code ?? [];
            var words = Enumerable.Range(0, code.Length / BytesPerDword)
                .Select(index => Hlsl.Hex(BinaryPrimitives.ReadUInt32LittleEndian(code.AsSpan(index * BytesPerDword)))).ToList();
            lines.Insert(1, $"static const uint g_code[{words.Count}] = {{{string.Join(",", words)}}};");
            lines.Insert(CodeLoadLine, $"uint CodeLoad(uint a) {{ return g_code[min(a >> 2, {words.Count - 1}u)]; }}");
        }
    }

    /// <summary>Program-wide rewrites: exact f16 rounding, helper lanes and rasterizer-ordered views for pixel shaders.</summary>
    private string Finish(string text)
    {
        // AMD rounds f32 to f16 to nearest even; HLSL f32tof16 rounding is implementation-defined.
        text = F32ToF16Pattern().Replace(text, "f32tof16_rne(");

        if (_state.Stage != ShaderStage.Pixel)
        {
            return text;
        }

        // EXEC starts with live pixels only; wave operations must still see helper lanes (needs ps_6_7).
        text = text.Replace("bool active = true", "bool active = !IsHelperLane()", StringComparison.Ordinal);

        if (_state.Pops || (_state.Config.Pixel?.Pops ?? false))
        {
            // Primitive-ordered pixel shading: every writable resource becomes a rasterizer-ordered view.
            text = WritableViewPattern().Replace(text, "RasterizerOrdered$1");
        }

        return text.Replace("\nOut main(", "\n[WaveOpsIncludeHelperLanes]\nOut main(", StringComparison.Ordinal);
    }

    [GeneratedRegex(@"^\s*pc = \(.*\) \? 0x[0-9a-f]+ : 0x[0-9a-f]+;\s*$")]
    private static partial Regex BranchPattern();

    [GeneratedRegex(@"\bf32tof16\(")]
    private static partial Regex F32ToF16Pattern();

    [GeneratedRegex(@"\bRW(Texture[123]D(?:Array)?|ByteAddressBuffer|Buffer|StructuredBuffer)\b")]
    private static partial Regex WritableViewPattern();
}
