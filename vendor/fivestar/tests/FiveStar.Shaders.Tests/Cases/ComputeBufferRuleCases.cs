// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// One case per buffer range rule (docs/shader-translation.md, Range checking): OOB_SELECT 0 (index and offset + size
/// against the stride, soffset not counted), 1 (index only), 2 (num_records 0 or nothing), the format element against
/// the stride, the d16 forms of formatted loads and stores, and an atomic past a raw buffer's end. A dword past the
/// range reads 0, a store or atomic there is dropped and an atomic returns 0.
/// </summary>
public static class ComputeBufferRuleCases
{
    private const int Threads = 64;
    private const int DataHeap = 3;
    private const int WrittenHeap = 4;
    private const uint DwordSelect = 0xFAC;
    private const int FormatShift = 12;
    private const int SelectShift = 28;
    private const int StrideShift = 16;

    /// <summary>GFX10 unified buffer formats (prelude FMT_UNIFIED).</summary>
    private const uint Format16Uint = 11;
    private const uint Format16Float = 13;
    private const uint Format16x4Float = 71;
    private const uint Format32x4Uint = 75;
    private const uint Format8x4Uint = 60;

    /// <summary>DST_SEL z, y, x, 1 (a BGRA view with its alpha forced to one), and w, 0, 0, 0.</summary>
    private const uint BgraOneSelect = 6 | (5 << 3) | (4 << 6) | (1 << 9);
    private const uint WOnlySelect = 7;

    private static readonly uint[] Input = [.. Enumerable.Range(0, 1024).Select(index => 0xA0000000 + (0x11 * (uint)index))];

    /// <summary>f16 halves 0x3C00 + i, two per dword: exact through f16 -> f32 -> f16.</summary>
    private static readonly uint[] Halves = [.. Enumerable.Range(0, 256).Select(index => (0x3C00 + (2 * (uint)index)) | ((0x3C00 + (2 * (uint)index) + 1) << 16))];

    public static IEnumerable<ComputeCase> All()
    {
        return [Structured(), Unchecked(), FormatElements(), FormatSelects(), HalfData(), AtomicPastEnd()];
    }

    /// <summary>A V# of heap slot <paramref name="heap"/> as the hardware reads it.</summary>
    private static uint[] Descriptor(int heap, uint stride, uint records, uint select, uint format = 0, uint destination = DwordSelect)
    {
        return [(uint)heap, stride << StrideShift, records, destination | (format << FormatShift) | (select << SelectShift)];
    }

    // OOB_SELECT 0, stride 8, 4 records: a dword at offset 8 is past the stride, a dwordx2 at 4 keeps its first dword
    // (each dword is checked), soffset moves the address but not the check. OOB_SELECT 1 checks the index only.
    private static ComputeCase Structured()
    {
        const string Program = """
            buffer_load_dword v20, v41, s[4:7], 0 idxen
            buffer_load_dword v21, v41, s[4:7], 0 idxen offset:4
            buffer_load_dword v22, v41, s[4:7], 0 idxen offset:8
            buffer_load_dwordx2 v[23:24], v41, s[4:7], 0 idxen offset:4
            s_mov_b32 s26, 16
            buffer_load_dword v25, v41, s[4:7], s26 idxen
            buffer_load_dword v26, v41, s[8:11], 0 idxen offset:8
            s_waitcnt vmcnt(0)
            """;
        const int Records = 4;

        return new ComputeCase("cs_buffer_rule_structured", Program, 7, thread =>
        {
            uint Word(int index) => Inside(thread.Id < Records, Input[index]);
            int t = thread.Id;

            return [.. new[] { Word(2 * t), Word((2 * t) + 1), 0u, Word((2 * t) + 1), 0u, Word(4 + (2 * t)), Word((2 * t) + 2) }.Select(Expected.Exact)];
        })
        {
            SecondDescriptor = Descriptor(DataHeap, 8, Records, 0),
            User = Descriptor(DataHeap, 8, Records, 1),
            Buffers = [new BufferResource(DataHeap, Input, Writable: false)],
        };
    }

    // OOB_SELECT 2: num_records 0 reads nothing, any other count everything (index past it included). s_buffer_load
    // counts num_records x stride bytes.
    private static ComputeCase Unchecked()
    {
        const string Program = """
            buffer_load_dword v20, v41, s[4:7], 0 idxen
            buffer_load_dword v21, v41, s[8:11], 0 idxen
            s_buffer_load_dword s24, s[8:11], 0x0
            s_buffer_load_dword s25, s[8:11], 0x4
            s_waitcnt vmcnt(0) lgkmcnt(0)
            v_mov_b32_e32 v22, s24
            v_mov_b32_e32 v23, s25
            """;

        return new ComputeCase("cs_buffer_rule_unchecked", Program, 4, thread => [Expected.Exact(0), Expected.Exact(Input[thread.Id]), Expected.Exact(Input[0]), Expected.Exact(0)])
        {
            SecondDescriptor = Descriptor(DataHeap, 4, 0, 2),
            User = Descriptor(DataHeap, 4, 1, 2),
            Buffers = [new BufferResource(DataHeap, Input, Writable: false)],
        };
    }

    // A format access checks its element against the stride: R16 records of stride 2 are in range (a dword check failed
    // every one), a 16-byte element in a stride of 12 never is.
    private static ComputeCase FormatElements()
    {
        const string Program = """
            buffer_load_format_x v20, v41, s[4:7], 0 idxen
            buffer_load_format_xyzw v[21:24], v41, s[8:11], 0 idxen
            s_waitcnt vmcnt(0)
            """;
        const int Records = 8;

        return new ComputeCase("cs_buffer_rule_format", Program, 5, thread =>
        {
            var half = thread.Id < Records ? (Input[thread.Id / 2] >> (16 * (thread.Id % 2))) & 0xFFFF : 0u;

            return [Expected.Exact(half), Expected.Exact(0), Expected.Exact(0), Expected.Exact(0), Expected.Exact(0)];
        })
        {
            SecondDescriptor = Descriptor(DataHeap, 2, Records, 0, Format16Uint),
            User = Descriptor(DataHeap, 12, Threads, 0, Format32x4Uint),
            Buffers = [new BufferResource(DataHeap, Input, Writable: false)],
        };
    }

    // DST_SEL picks the fields of an 8_8_8_8_UINT element: z, y, x and the constant 1 for a BGRA-ordered view, and the
    // fourth field in x with zero after it, so the selected field and not the result slot decides each value.
    private static ComputeCase FormatSelects()
    {
        const string Program = """
            buffer_load_format_xyzw v[20:23], v41, s[4:7], 0 idxen
            buffer_load_format_xy v[24:25], v41, s[8:11], 0 idxen
            s_waitcnt vmcnt(0)
            """;

        return new ComputeCase("cs_buffer_rule_format_select", Program, 6, thread =>
        {
            uint Field(int index) => (Input[thread.Id] >> (8 * index)) & 0xFF;

            return [.. new[] { Field(2), Field(1), Field(0), 1u, Field(3), 0u }.Select(Expected.Exact)];
        })
        {
            SecondDescriptor = Descriptor(DataHeap, 4, Threads, 0, Format8x4Uint, BgraOneSelect),
            User = Descriptor(DataHeap, 4, Threads, 0, Format8x4Uint, WOnlySelect),
            Buffers = [new BufferResource(DataHeap, Input, Writable: false)],
        };
    }

    // The d16 forms: buffer_load_format_d16_x (16_FLOAT, stride 2) writes the low half and keeps the high one,
    // d16_xyzw (16_16_16_16_FLOAT, stride 8) packs four halves into two VGPRs, and buffer_store_format_d16_xyzw stores
    // them back unchanged through a copy of that V# naming the written buffer.
    private static ComputeCase HalfData()
    {
        const string Program = """
            v_mov_b32 v20, 0x12340000
            buffer_load_format_d16_x v20, v41, s[4:7], 0 idxen
            buffer_load_format_d16_xyzw v[21:22], v41, s[8:11], 0 idxen
            s_waitcnt vmcnt(0)
            s_mov_b32 s24, 4
            s_mov_b32 s25, s9
            s_mov_b32 s26, s10
            s_mov_b32 s27, s11
            buffer_store_format_d16_xyzw v[21:22], v41, s[24:27], 0 idxen
            """;
        var stored = Halves.Take(2 * Threads).ToArray();

        return new ComputeCase("cs_buffer_rule_d16", Program, 3, thread =>
        {
            var half = (Halves[thread.Id / 2] >> (16 * (thread.Id % 2))) & 0xFFFF;

            return [Expected.Exact(0x12340000 | half), Expected.Exact(Halves[2 * thread.Id]), Expected.Exact(Halves[(2 * thread.Id) + 1])];
        })
        {
            SecondDescriptor = Descriptor(DataHeap, 2, 2 * Threads, 1, Format16Float),
            User = Descriptor(DataHeap, 8, Threads, 1, Format16x4Float),
            Buffers = [new BufferResource(DataHeap, Halves, Writable: false), new BufferResource(WrittenHeap, new uint[2 * Threads], Writable: true)],
            Check = buffers => Differences($"heap{WrittenHeap}", stored, buffers[WrittenHeap]),
        };
    }

    // An atomic past a raw buffer's end (16 bytes) is dropped and returns 0; inside it returns the old value.
    private static ComputeCase AtomicPastEnd()
    {
        const string Program = """
            v_mov_b32 v20, 1
            v_lshlrev_b32_e32 v1, 2, v41
            buffer_atomic_add v20, v1, s[4:7], 0 offen glc
            s_waitcnt vmcnt(0)
            """;
        const int Bytes = 16;
        uint[] initial = [.. Enumerable.Range(0, Threads).Select(index => 100 + (uint)index)];
        var after = initial.ToArray();

        for (var index = 0; index < Bytes / 4; index++)
        {
            after[index]++;
        }

        return new ComputeCase("cs_buffer_rule_atomic", Program, 1, thread => [Expected.Exact(Inside(thread.Id < Bytes / 4, initial[thread.Id]))])
        {
            SecondDescriptor = Descriptor(WrittenHeap, 0, Bytes, 3),
            Buffers = [new BufferResource(WrittenHeap, initial, Writable: true)],
            Check = buffers => Differences($"heap{WrittenHeap}", after, buffers[WrittenHeap]),
        };
    }

    /// <summary>What an access reads: the value inside the range, 0 outside.</summary>
    private static uint Inside(bool inside, uint value)
    {
        if (inside)
        {
            return value;
        }

        return 0;
    }

    private static IEnumerable<string> Differences(string name, uint[] expected, uint[] actual)
    {
        for (var index = 0; index < expected.Length; index++)
        {
            if (expected[index] != actual[index])
            {
                yield return string.Create(CultureInfo.InvariantCulture, $"{name}[{index}] expected {Expected.Hex(expected[index])} actual {Expected.Hex(actual[index])}");
            }
        }
    }
}
