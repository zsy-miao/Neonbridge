// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// s_sendmsg: GS_ALLOC_REQ (mesh output counts from M0), GS EMIT/CUT (geometry output stream), ORDERED_PS_DONE (end of a
/// POPS critical section). Messages with no effect in this model are dropped.
/// </summary>
internal sealed partial class MessageEmitter
{
    private readonly TranslationState _state;

    public MessageEmitter(TranslationState state)
    {
        _state = state;
    }

    public void Translate(string operands)
    {
        var name = MessageNamePattern().Match(operands);
        var message = name.Success ? name.Groups[1].Value : operands;

        switch (message)
        {
            case "MSG_GS_ALLOC_REQ" when _state.Stage == ShaderStage.Mesh:
                AllocateMeshOutputs();
                break;
            case "MSG_GS" when _state.Stage == ShaderStage.Geometry:
                GeometryStream(operands);
                break;
            case "MSG_ORDERED_PS_DONE":
                _state.Pops = true;
                break;
            case "MSG_GS_DONE":
            case "MSG_DEALLOC_VGPRS":
            case "MSG_GS":
            case "MSG_SAVEWAVE":
                break;
            default:
                throw new UnsupportedProgramException($"s_sendmsg {message}");
        }
    }

    private void AllocateMeshOutputs()
    {
        // GS_ALLOC_REQ: M0 = primitive count << 12 | vertex count.
        _state.Variables.Add(RegisterKey.Sgpr(SpecialRegisters.M0));
        _state.Emit("  if (WaveIsFirstLane()) { g_ms_nv = s_m0 & 0xfffu; g_ms_np = (s_m0 >> 12) & 0xfffu; g_ms_alloc = 1; }");
    }

    private void GeometryStream(string operands)
    {
        var match = GeometryMessagePattern().Match(operands);

        if (!match.Success || match.Groups["stream"].Value != "0")
        {
            throw new UnsupportedProgramException($"s_sendmsg GS stream {operands}");
        }

        switch (match.Groups["op"].Value)
        {
            case "GS_OP_EMIT":
                _state.Emit("  if (active) { gs_os.Append(gs_copy(gs_emit * GS_VTX_DW, 0u)); gs_emit++; }");
                break;
            case "GS_OP_CUT":
                _state.Emit("  if (active) gs_os.RestartStrip();");
                break;
            default:
                throw new UnsupportedProgramException($"s_sendmsg {match.Groups["op"].Value}");
        }
    }

    [GeneratedRegex(@"sendmsg\((\w+)")]
    private static partial Regex MessageNamePattern();

    [GeneratedRegex(@"sendmsg\(MSG_GS, (?<op>GS_OP_\w+), (?<stream>\d+)\)")]
    private static partial Regex GeometryMessagePattern();
}
