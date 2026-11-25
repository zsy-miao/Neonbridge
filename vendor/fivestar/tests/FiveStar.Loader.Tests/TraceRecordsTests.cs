// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Text;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--d3d12-trace-records: the window records a program reads through user SGPR pointers.</summary>
public sealed unsafe class TraceRecordsTests
{
    private const uint WindowBytes = 0x1000;
    private const uint Table = 0x200;

    [Fact]
    public void Records_at_sgpr_pointers_show_a_V_sharp_base_and_values_a_T_sharp_base_and_a_heap_index()
    {
        var offsets = TraceRecords.Parse("0x20@0x24, 64 ,0x60");
        Assert.Equal([new TraceRecord(0x20, 0x24), new TraceRecord(0x40), new TraceRecord(0x60)], offsets);
        Assert.Equal([new TraceRecord(0x380, 0x11c), new TraceRecord(32)], TraceRecords.Parse("0x380 @ 0x11c,32"));

        byte* window = (byte*)NativeMemory.AllocZeroed(WindowBytes);
        uint* constants = (uint*)NativeMemory.AllocZeroed(TraceRecords.BaseDwords * sizeof(uint));
        uint* gameRecord = (uint*)NativeMemory.AllocZeroed(TraceRecords.RecordDwords * sizeof(uint));

        try
        {
            // +0x20: a V# at console 0x284_8a43b440, 4336 records; +0x40: a T# (type 9) at 0x12_3456_7800; +0x60: a heap index.
            Write(window + Table + 0x20, [0x8a43b440, 0x284, 0x10f0, 0x31014fac]);
            Write(window + Table + 0x40, [0x12345678, 0x00000000, 0, 0x90000000]);
            Write(window + Table + 0x60, [417733, 0, 1, 0x11014fac]);

            gameRecord[0] = 0x7787000;
            gameRecord[3] = 0x9000f000;
            constants[0] = 0x40000000;
            constants[2] = 0xbf800000;

            var text = new StringBuilder();
            uint[] sgprs = [Table, 0x123, 0, Table, WindowBytes];
            var mapped = new Dictionary<ulong, string> { [0x2848a43b440] = "mapped" };
            // The buffer's dword at byte 0x24 (past the first 10 shown) is a garbage loop count.
            uint count = 0xffffff00;
            var memory = new Dictionary<ulong, nint> { [0x2848a43b440] = (nint)constants, [0x2848a43b440 + 0x24] = (nint)(&count) };

            // The game's heap holds a T# where the window's +0x60 record is; the other records have no game heap behind them.
            var game = new Dictionary<uint, nint> { [Table + 0x60] = (nint)gameRecord };
            var sources = new TraceRecordSources(address => mapped.GetValueOrDefault(address, "UNMAPPED"), (address, _) => memory.GetValueOrDefault(address), offset => game.GetValueOrDefault(offset), address => $"texture at 0x{address:x}", index => $"SRV {index}");
            TraceRecords.Append(text, window, WindowBytes, sgprs, offsets, sources);
            string log = text.ToString();

            // Only sgpr 0 is a usable window offset: 0x123 is not record-aligned, sgpr 3 repeats it, 0x1000 has no room.
            Assert.Equal(3, log.Split("\n      sgpr ").Length - 1);
            Assert.Contains("sgpr 0 0x200 +0x20: 8a43b440 00000284 000010f0 31014fac", log, StringComparison.Ordinal);
            Assert.Contains("V# base 0x2848a43b440, 4336 records: mapped; first dwords: 40000000 00000000 bf800000", log, StringComparison.Ordinal);

            // Only the entry naming a dword shows one.
            Assert.Contains("; dword at +0x24: ffffff00 (4294967040)", log, StringComparison.Ordinal);
            Assert.Single(log.Split("dword at +").Skip(1));

            Assert.Contains("T# base 0x1234567800: UNMAPPED; texture at 0x1234567800", log, StringComparison.Ordinal);
            Assert.Contains("heap index 417733, no console base: SRV 417733", log, StringComparison.Ordinal);

            Assert.Contains("game record 19: 07787000 00000000 00000000 9000f000", log, StringComparison.Ordinal);
            Assert.Contains("game record 17: (no game heap)", log, StringComparison.Ordinal);
        }
        finally
        {
            NativeMemory.Free(window);
            NativeMemory.Free(constants);
            NativeMemory.Free(gameRecord);
        }
    }

    private static void Write(byte* at, uint[] dwords)
    {
        dwords.CopyTo(new Span<uint>(at, dwords.Length));
    }
}
