// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// The view and byte address of a MUBUF/MTBUF access. The view comes from the V#'s base and the access's offset
/// (prelude <c>buf_view</c>): a heap index while base_hi is 0, else the console GPU address base + offset, looked up in
/// game memory. The offset is soffset + instruction offset, plus index * stride (idxen, stride in V# dword1 bits 16-29)
/// and/or the VGPR offset (offen); with both, vaddr holds the index then the offset.
/// </summary>
internal static class BufferAddress
{
    private const int BaseDword = 0;
    private const int StrideDword = 1;
    private const int RecordsDword = 2;
    private const int FormatDword = 3;

    /// <summary>
    /// A uint2 temporary holding the heap index (x) and the byte address in that view (y) of the access at byte
    /// <paramref name="offset"/> from the V#'s base (<see cref="Build"/>, or an s_buffer_load offset).
    /// </summary>
    public static string View(OperandAccess operands, Operand resource, bool writable, string offset)
    {
        var rw = writable ? "true" : "false";

        return operands.SnapPair($"buf_view({operands.Read(resource, BaseDword)}, {operands.Read(resource, StrideDword)}, {rw}, {offset})");
    }

    public static string Build(OperandAccess operands, Operand vaddr, Operand resource, Operand soffset, InstructionModifiers modifiers)
    {
        var address = $"({operands.Read(soffset)} + {modifiers.Integer("offset", 0)}u";
        var indexed = modifiers.Has("idxen");
        var offset = modifiers.Has("offen");

        if (indexed && offset)
        {
            var index = operands.Read(vaddr, Low);
            address += $" + {index} * {Stride(operands, resource)} + {operands.Read(vaddr, High)}";
        }
        else if (indexed)
        {
            var index = operands.Read(vaddr);
            address += $" + {index} * {Stride(operands, resource)}";
        }
        else if (offset)
        {
            address += $" + {operands.Read(vaddr)}";
        }

        return address + ")";
    }

    /// <summary>
    /// The checked offset of a MUBUF/MTBUF access (VGPR offset + instruction offset, plus soffset for a raw V#; prelude
    /// buf_offset) and the end its dwords must fit under (buf_limit), as snapped temporaries for <see cref="InRange"/>.
    /// </summary>
    public static (string Offset, string Limit) Range(OperandAccess operands, Operand vaddr, Operand resource, Operand soffset, InstructionModifiers modifiers)
    {
        var indexed = modifiers.Has("idxen");
        var offset = modifiers.Has("offen");
        var index = "0u";
        var bytes = $"{modifiers.Integer("offset", 0)}u";

        if (indexed && offset)
        {
            index = operands.Read(vaddr, Low);
            bytes += $" + {operands.Read(vaddr, High)}";
        }
        else if (indexed)
        {
            index = operands.Read(vaddr);
        }
        else if (offset)
        {
            bytes += $" + {operands.Read(vaddr)}";
        }

        var limit = $"buf_limit({operands.Read(resource, StrideDword)}, {operands.Read(resource, RecordsDword)}, {operands.Read(resource, FormatDword)}, {index})";
        var checkedOffset = $"buf_offset({operands.Read(resource, FormatDword)}, {bytes}, {operands.Read(soffset)})";
        var values = operands.Snap(checkedOffset, limit);

        return (values[0], values[1]);
    }

    /// <summary>True when the dword <paramref name="extraBytes"/> past the access's offset is in range.</summary>
    public static string InRange((string Offset, string Limit) range, int extraBytes)
    {
        if (extraBytes == 0)
        {
            return $"buf_fits({range.Offset}, {range.Limit})";
        }

        return $"buf_fits({range.Offset} + {extraBytes}u, {range.Limit})";
    }

    private static string Stride(OperandAccess operands, Operand resource)
    {
        return $"buf_stride({operands.Read(resource, StrideDword)})";
    }
}
