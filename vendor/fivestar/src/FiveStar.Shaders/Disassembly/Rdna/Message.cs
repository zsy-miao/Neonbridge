// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The <c>sendmsg(...)</c> operand of s_sendmsg: message [3:0], operation [6:4], stream [9:8]; message and operation
/// names from the AMD "RDNA 2 Instruction Set Architecture" reference guide (S_SENDMSG messages). Combinations without
/// a name print as numbers, and an immediate with bits the fields do not cover prints as plain decimal.
/// </summary>
internal static class Message
{
    private const int IdWidth = 4;
    private const int OperationLow = 4;
    private const int OperationWidth = 3;
    private const int StreamLow = 8;
    private const int StreamWidth = 2;
    private const int Gs = 2;
    private const int GsDone = 3;
    private const int SystemMessage = 15;
    private const int GsNop = 0;

    private static readonly Dictionary<int, string> Names = new()
    {
        [1] = "MSG_INTERRUPT", [2] = "MSG_GS", [3] = "MSG_GS_DONE", [4] = "MSG_SAVEWAVE", [5] = "MSG_STALL_WAVE_GEN",
        [6] = "MSG_HALT_WAVES", [7] = "MSG_ORDERED_PS_DONE", [8] = "MSG_EARLY_PRIM_DEALLOC", [9] = "MSG_GS_ALLOC_REQ",
        [10] = "MSG_GET_DOORBELL", [11] = "MSG_GET_DDID", [15] = "MSG_SYSMSG",
    };

    private static readonly string[] GsOperations = ["GS_OP_NOP", "GS_OP_CUT", "GS_OP_EMIT", "GS_OP_EMIT_CUT"];

    private static readonly string[] SystemOperations =
        [string.Empty, "SYSMSG_OP_ECC_ERR_INTERRUPT", "SYSMSG_OP_REG_RD", "SYSMSG_OP_HOST_TRAP_ACK", "SYSMSG_OP_TTRACE_PC"];

    public static string Text(int immediate)
    {
        var word = (uint)immediate;
        var id = Bits.Field(word, 0, IdWidth);
        var operation = Bits.Field(word, OperationLow, OperationWidth);
        var stream = Bits.Field(word, StreamLow, StreamWidth);
        var symbolic = Symbolic(id, operation, stream);

        if (symbolic is not null)
        {
            return symbolic;
        }

        if ((id | (operation << OperationLow) | (stream << StreamLow)) == immediate)
        {
            return string.Create(CultureInfo.InvariantCulture, $"sendmsg({id}, {operation}, {stream})");
        }

        return immediate.ToString(CultureInfo.InvariantCulture);
    }

    private static string? Symbolic(int id, int operation, int stream)
    {
        if (!Names.TryGetValue(id, out var name))
        {
            return null;
        }

        if (id is Gs or GsDone)
        {
            return GsText(name, id, operation, stream);
        }

        if (id == SystemMessage)
        {
            if (operation == 0 || operation >= SystemOperations.Length || stream != 0)
            {
                return null;
            }

            return $"sendmsg({name}, {SystemOperations[operation]})";
        }

        if (operation != 0 || stream != 0)
        {
            return null;
        }

        return $"sendmsg({name})";
    }

    private static string? GsText(string name, int id, int operation, int stream)
    {
        if (operation >= GsOperations.Length || (id == Gs && operation == GsNop))
        {
            return null;
        }

        if (operation == GsNop)
        {
            if (stream != 0)
            {
                return null;
            }

            return $"sendmsg({name}, {GsOperations[operation]})";
        }

        return string.Create(CultureInfo.InvariantCulture, $"sendmsg({name}, {GsOperations[operation]}, {stream})");
    }
}
