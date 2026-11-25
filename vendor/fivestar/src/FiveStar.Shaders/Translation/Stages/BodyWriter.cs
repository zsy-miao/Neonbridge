// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using FiveStar.Shaders.Translation.Structure;
using FiveStar.Shaders.Translation.Waves;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// Writes the entry point body: register declarations (uint per register, bool per lane mask, <c>active</c> for EXEC),
/// the prologue of hardware inputs, then the blocks. With control flow the blocks run in a wave-uniform dispatch loop
/// over <c>pc</c>, or as structured if/else and loops when that option is set and the graph allows it; a two-phase mesh
/// shader runs its body twice around SetMeshOutputCounts.
/// </summary>
internal sealed class BodyWriter
{
    private const string Indent = "    ";

    // End of a mesh phase loop: the barrier at the top level for every thread of the group, then resume after it. On
    // the console s_barrier waits only for waves that have not ended (s_endpgm); a D3D12 group barrier waits for every
    // thread, so a thread that ended keeps meeting the barrier (counted out of g_ms_live) until no thread runs: a
    // console wave that ends while another of its group meets an s_barrier would otherwise leave that barrier unfilled
    // (a group hang). The counters rotate as vw_ss_step's do. The rounds are bounded (LoopCaps.Rounds): a console loop
    // around an s_barrier runs one round per iteration, which no structured loop counts, so a garbage trip count held
    // the group for good. ms_step is the same on every thread, so all leave on the same round.
    private string[] BarrierStep =>
    [
        "    if (pc != PC_END) InterlockedAdd(g_ms_live[ms_step % 3u], 1u);",
        "    if (in_gi == 0) g_ms_live[(ms_step + 1u) % 3u] = 0;",
        "    GroupMemoryBarrierWithGroupSync();",
        "    uint ms_live = g_ms_live[ms_step % 3u];",
        "    ms_step++;",
        "    if (ms_live == 0u) break;",
        $"    if (ms_step > {LoopGuard.RoundsHex(_state.Stage, _state.Loops)}) {{ if (in_gi == 0) ss_cap_hit(1u); break; }}",
        "    if (pc != PC_END) pc = pc_resume;",
    ];

    private string[] PhaseTwoEnd => [.. BarrierStep, "  }"];

    // Phase two when none of its blocks yields: the barrier could never run, and NVIDIA's driver (RTX 5070, 621.40) hangs
    // the group on dxc's optimized form of the program while it is there.
    private static readonly string[] PhaseTwoEndWithoutYield =
    [
        "    if (pc == PC_END) break;", "    pc = pc_resume;", "  }",
    ];

    // Phase one also stops once the output counts are known, all threads on the flag read before any can set it again.
    private string[] PhaseOneEnd =>
    [
        .. BarrierStep, "    bool ms_alloc = g_ms_alloc != 0;", "    GroupMemoryBarrierWithGroupSync();", "    if (ms_alloc) break;", "  }",
    ];

    private readonly TranslationState _state;
    private readonly MeshDeclarations _mesh;
    private readonly StructuredBodyWriter _structured;
    private readonly bool _useStructured;

    public BodyWriter(TranslationState state, MeshDeclarations mesh, StructuredBodyWriter structured, bool useStructured)
    {
        _state = state;
        _mesh = mesh;
        _structured = structured;
        _useStructured = useStructured;
    }

    public void Write(List<string> lines)
    {
        WriteDeclarations(lines);

        if (_state.Supersteps)
        {
            lines.AddRange(SuperstepWriter.Body(_state, _state.Hoisted, _mesh));
        }
        else if (!_state.HasControlFlow)
        {
            foreach (var block in _state.Blocks)
            {
                lines.AddRange(block.Lines.Where(line => !line.Contains("pc = ", StringComparison.Ordinal)));
            }
        }
        else if (_useStructured)
        {
            WriteStructured(lines);
        }
        else
        {
            WriteDispatch(lines);
        }

        WriteEpilogue(lines);
    }

    private void WriteDeclarations(List<string> lines)
    {
        var stage = _state.Stage;

        if (stage == ShaderStage.Mesh)
        {
            lines.Add("  if (in_gi == 0) { g_ms_nv = 0; g_ms_np = 0; g_ms_alloc = 0; }");

            if (_state.MeshYield)
            {
                lines.AddRange(["  if (in_gi == 0) { g_ms_live[0] = 0; g_ms_live[1] = 0; g_ms_live[2] = 0; }", "  uint ms_step = 0;"]);
            }

            if (!_state.MeshYield)
            {
                lines.Add("  g_ms_prim[in_gi] = 0x80000000u;");
            }

            lines.Add("  GroupMemoryBarrierWithGroupSync();");

            if (_state.EmulateWaves)
            {
                lines.Add($"  {WaveOps.GroupLane} = in_gi & (VW_WAVE - 1u);");
            }
        }

        if (stage == ShaderStage.Compute && _state.Supersteps)
        {
            // Console waves are consecutive group indices, as for a mesh program.
            lines.Add($"  {WaveOps.GroupLane} = in_gi & (VW_WAVE - 1u);");
        }
        else if (stage == ShaderStage.Compute)
        {
            // The wave32 variant of a wave64 program (HlslTranslator.GroupWaveVariant) runs the group as the wave.
            lines.AddRange(["#if VW_GROUP_WAVE", "  vw_begin(in_gi);", "#endif"]);
        }

        if (stage == ShaderStage.Geometry)
        {
            lines.Add("  uint gs_emit = 0;");
        }
        else
        {
            var exports = stage is ShaderStage.Compute or ShaderStage.Mesh ? "  Exp x = (Exp)0;" : "  Out o = (Out)0; Exp x = (Exp)0;";
            lines.Add(exports);
        }

        lines.AddRange(_state.InputMap);
        var registers = _state.Variables.Order().Select(key => $"{key.Name} = 0").ToList();

        if (registers.Count > 0)
        {
            lines.Add("  uint " + string.Join(", ", registers) + ";");
        }

        var masks = _state.Masks.Union(_state.MasksEver).Where(key => key != SpecialRegisters.Exec)
            .Select(key => key.MaskName).Distinct().Order(StringComparer.Ordinal).Select(name => $", {name} = false");
        lines.Add("  bool active = true, scc = false" + string.Concat(masks) + ";");
        lines.AddRange(_state.Prologue);
    }

    private bool IsTwoPhaseMesh => _state.MeshYield && _state.Stage == ShaderStage.Mesh;

    /// <summary>The structured form; the dispatch form (with a comment naming the reason) when the graph does not allow it.</summary>
    private void WriteStructured(List<string> lines)
    {
        List<string> body;
        lines.Add("  uint loop_budget = 0;");

        try
        {
            body = IsTwoPhaseMesh ? Phases(_structured.Phase) : _structured.Program();
        }
        catch (StructuringFailedException exception)
        {
            lines.Add($"  // control flow: dispatch loop ({exception.Message})");
            WriteDispatch(lines);

            return;
        }

        lines.AddRange(body);
    }

    private void WriteDispatch(List<string> lines)
    {
        if (!IsTwoPhaseMesh)
        {
            lines.Add("  uint pc = 0, pc_resume = 0;");
            lines.AddRange(Dispatch(dropOutputs: false).Select(FenceOnly));

            return;
        }

        lines.AddRange(Phases(Dispatch));
    }

    /// <summary>
    /// An s_barrier inside the dispatch switch keeps only its memory fence: each thread runs the switch on its own, so a
    /// thread that ended (pc = PC_END) or sits in another block never reaches the barrier others wait in, and the group
    /// hangs (XCL counts live waves, VIctory keeps only the fence where not every lane reaches the barrier; facts only).
    /// The console's s_barrier waits for the waves that have not ended. Only programs whose control flow could not be
    /// structured take this form (4 of 400 compute programs sampled hold a barrier there); WARP's compiler also crashed
    /// on such barriers.
    /// </summary>
    private static string FenceOnly(string line)
    {
        return line.Replace("GroupMemoryBarrierWithGroupSync();", "GroupMemoryBarrier();   // dispatch switch: fence only", StringComparison.Ordinal);
    }

    /// <summary>
    /// Phase 1 runs without output writes until the first barrier after GS_ALLOC_REQ; SetMeshOutputCounts then
    /// dominates every output write; phase 2 resumes with direct exports. Barriers stay at the top level of the outer
    /// loop (a barrier inside the switch, even in a block never re-entered, crashes WARP's shader compiler). Phase 2 has
    /// a barrier only when a block it can run yields (<see cref="PhaseTwoYields"/>).
    /// </summary>
    private List<string> Phases(Func<bool, List<string>> phase)
    {
        var lines = new List<string> { "  uint pc = 0, pc_resume = 0;", "  [loop] for (;;) {" };
        lines.AddRange(phase(true));
        lines.AddRange(PhaseOneEnd);

        lines.Add($"  SetMeshOutputCounts(min(g_ms_nv, {_mesh.MaxVertices}u), min(g_ms_np, {_mesh.MaxPrimitives}u));");
        lines.AddRange(_mesh.InitialPrimitives());
        lines.Add("  [loop] for (;;) {");
        lines.AddRange(phase(false));

        if (PhaseTwoYields())
        {
            lines.AddRange(PhaseTwoEnd);
            lines.AddRange(_mesh.CutEpilogue($"ms_step > {LoopGuard.RoundsHex(_state.Stage, _state.Loops)}"));
        }
        else
        {
            lines.AddRange(PhaseTwoEndWithoutYield);
        }

        return lines;
    }

    /// <summary>
    /// Whether phase 2 can reach a yield: it starts at the resume points of the yields (phase 1 ends after a barrier),
    /// and runs from each through the graph to the next yield or the end.
    /// </summary>
    private bool PhaseTwoYields()
    {
        var graph = new FlowGraph(_state.Blocks);
        var pending = new Stack<int>();
        var seen = new HashSet<int>();

        foreach (var block in _state.Blocks.Where(block => block.Exit.Kind == BlockExitKind.Yield))
        {
            var resume = block.Exit.Target is null ? null : graph.NodeAt(block.Exit.Target.Value);

            if (resume is not null)
            {
                pending.Push(resume.Value);
            }
        }

        while (pending.Count > 0)
        {
            var node = pending.Pop();

            if (node == graph.End || !seen.Add(node))
            {
                continue;
            }

            if (graph.Blocks[node].Exit.Kind == BlockExitKind.Yield)
            {
                return true;
            }

            foreach (var next in graph.Successors[node])
            {
                pending.Push(next);
            }
        }

        return false;
    }

    /// <summary>
    /// The wave-uniform block dispatch: every lane follows the same pc; divergence lives in <c>active</c>. It stops after
    /// <see cref="LoopGuard.DispatchSteps"/> blocks.
    /// </summary>
    private List<string> Dispatch(bool dropOutputs)
    {
        var lines = new List<string> { $"  [loop] for (uint pc_steps = 0; pc < PC_YIELD && pc_steps < {LoopGuard.DispatchHex(_state.Stage, _state.Loops)}; pc_steps++) {{", "    switch (pc) {" };

        foreach (var block in _state.Blocks)
        {
            lines.Add($"    case {Hlsl.Hex(block.Address)}: {{");

            foreach (var line in block.Lines)
            {
                if (!dropOutputs || !MeshDeclarations.WritesOutput(line))
                {
                    lines.Add(Indent + line);
                }
            }

            lines.Add("    } break;");
        }

        lines.Add("    default: pc = PC_END; break;");
        lines.Add("    }");
        lines.Add("  }");

        return lines;
    }

    private void WriteEpilogue(List<string> lines)
    {
        var stage = _state.Stage;

        if (stage is ShaderStage.Compute or ShaderStage.Geometry || (stage == ShaderStage.Mesh && _state.MeshYield))
        {
            lines.Add("}");

            return;
        }

        if (stage == ShaderStage.Mesh)
        {
            lines.AddRange(_mesh.StagingEpilogue());

            return;
        }

        lines.AddRange(_state.OutputMap);
        lines.Add("  return o;");
        lines.Add("}");
    }
}
