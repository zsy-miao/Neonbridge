// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using FiveStar.Shaders.Translation.Structure;

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>
/// The structured form of a program's blocks. A two-phase mesh shader keeps its phase loops: each phase switches on
/// <c>pc</c> over the program entry and the resume points after its barriers, and each case is the structured region
/// up to the next barrier or the end. The switch stays inside <c>[loop] while (pc &lt; PC_YIELD)</c> as in the dispatch
/// form although every case ends at a yield or the end: WARP's shader compiler crashes on the phase loop without it.
/// Throws <see cref="StructuringFailedException"/> when the graph cannot be structured.
/// </summary>
internal sealed class StructuredBodyWriter
{
    // Statements inside a case of the phase switch sit two levels below the function body.
    private const int CaseDepth = 2;

    // The dispatch loop starts at pc 0.
    private const long EntryAddress = 0;

    private readonly TranslationState _state;

    public StructuredBodyWriter(TranslationState state)
    {
        _state = state;
    }

    /// <summary>The whole program as structured statements.</summary>
    public List<string> Program()
    {
        var graph = new FlowGraph(_state.Blocks);
        var entry = graph.NodeAt(EntryAddress);

        if (entry is null)
        {
            return [];
        }

        return new RegionEmitter(graph, entry.Value, _ => true, LoopGuard.Hex(_state.Stage, _state.Loops)).Emit(0);
    }

    /// <summary>One mesh phase: a switch over the regions; phase 1 (<paramref name="dropOutputs"/>) writes no outputs.</summary>
    public List<string> Phase(bool dropOutputs)
    {
        var graph = new FlowGraph(_state.Blocks);
        var lines = new List<string> { "  [loop] while (pc < PC_YIELD) {", "    switch (pc) {" };
        Func<string, bool> keep = dropOutputs ? line => !MeshDeclarations.WritesOutput(line) : _ => true;

        foreach (var entry in RegionEntries(graph))
        {
            lines.Add($"    case {Hlsl.Hex(graph.Blocks[entry].Address)}: {{");
            lines.Add("      pc = PC_END;");
            lines.AddRange(new RegionEmitter(graph, entry, keep, LoopGuard.Hex(_state.Stage, _state.Loops)).Emit(CaseDepth));
            lines.Add("    } break;");
        }

        lines.Add("    default: pc = PC_END; break;");
        lines.Add("    }");
        lines.Add("  }");

        return lines;
    }

    /// <summary>The program entry and every block a barrier resumes at.</summary>
    private static List<int> RegionEntries(FlowGraph graph)
    {
        var entries = new List<int>();
        var entry = graph.NodeAt(EntryAddress);

        if (entry is not null)
        {
            entries.Add(entry.Value);
        }

        foreach (var block in graph.Blocks.Where(block => block.Exit.Kind == BlockExitKind.Yield))
        {
            var target = block.Exit.Target;
            var resume = target is null ? null : graph.NodeAt(target.Value);

            if (resume is not null && !entries.Contains(resume.Value))
            {
                entries.Add(resume.Value);
            }
        }

        return entries;
    }
}
