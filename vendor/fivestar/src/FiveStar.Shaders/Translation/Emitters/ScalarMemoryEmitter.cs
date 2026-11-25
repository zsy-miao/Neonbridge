// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// Scalar memory loads. s_buffer_load reads the V#'s view (<see cref="BufferAddress.View"/>); s_load reads through a
/// 64-bit pointer (prelude PtrLoad: the window for high dword 0, else game memory), or the program's own code when the
/// pointer came from s_getpc_b64 (high dword CODE_TAG). In geometry programs the driver's ring table pointer reads as 0.
/// The dword that loads an S#'s dword 0 reads the sampler heap's records for a 32-bit pointer (prelude SamplerPtrLoad,
/// <see cref="SamplerLoads"/>).
/// </summary>
internal sealed class ScalarMemoryEmitter
{
    private const int BaseOperand = 1;
    private const int OffsetOperand = 2;
    private const int StrideDword = 1;
    private const int RecordsDword = 2;
    private const int BytesPerDword = 4;

    private static readonly IReadOnlyDictionary<string, int> Sizes = new Dictionary<string, int>
    {
        ["dword"] = 1,
        ["dwordx2"] = 2,
        ["dwordx4"] = 4,
        ["dwordx8"] = 8,
        ["dwordx16"] = 16,
    };

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public ScalarMemoryEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    public void Translate(string op, IReadOnlyList<Operand> a)
    {
        var count = Sizes[op[(op.LastIndexOf('_') + 1)..]];
        var pointer = a[BaseOperand];
        var offsetValue = _operands.Read(a[OffsetOperand]);

        if (op.StartsWith("s_buffer_load", StringComparison.Ordinal))
        {
            var view = BufferAddress.View(_operands, pointer, false, offsetValue);
            var start = $"{view}.y";
            var checkedOffset = _operands.SnapOne(offsetValue);
            var limit = _operands.SnapOne($"sbuf_limit({_operands.Read(pointer, StrideDword)}, {_operands.Read(pointer, RecordsDword)})");

            // Out of range (past prelude sbuf_limit) reads 0; --trace-pool-stores logs such a load.
            _state.Emit($"  TRACE_SBUF({checkedOffset}, {count * BytesPerDword}u, {limit});");

            for (var dword = 0; dword < count; dword++)
            {
                _operands.Write(a[Destination], $"select({checkedOffset} + {dword * BytesPerDword}u < {limit}, BufLoad({view}.x, {start} + {dword}u * 4u), 0u)", dword);
            }

            return;
        }

        var values = _operands.Snap(_operands.Read(pointer, Low), offsetValue);
        var (address, offset) = (values[0], values[1]);
        var high = _operands.SnapOne(_operands.Read(pointer, High));

        for (var dword = 0; dword < count; dword++)
        {
            _operands.Write(a[Destination], PointerLoad(address, offset, high, dword), dword);
        }
    }

    private string PointerLoad(string address, string offset, string high, int dword)
    {
        var function = "PtrLoad";

        if (_state.SamplerLoadDwords.Contains((_state.Address, dword)))
        {
            function = "SamplerPtrLoad";
        }

        var load = $"{function}({address}, {high}, {offset} + {dword}u * 4u)";

        if (_state.UsesCode)
        {
            load = $"(({high}) == CODE_TAG ? CodeLoad({address} + {offset} + {dword}u * 4u) : {load})";
        }

        if (_state.Stage == ShaderStage.Geometry || _state.IsCopyShader)
        {
            // The driver ring table (GSVS base) reads as 0 in the private-LDS model.
            load = $"(({high}) == RING_TAG ? 0u : {load})";
        }

        return load;
    }
}
