// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;

namespace FiveStar.Loader.Cpu;

/// <summary>
/// Checks the emulation through the real exception path on any CPU. Each case is generated twice into the code heap
/// (never into game code): plain, which the CPU runs natively when it has the instruction and the handler emulates when
/// it does not, and marked with a preceding <c>ud2</c>, which faults on every CPU and which the handler emulates only
/// in this code. Both must agree, with each other and with <see cref="BitField"/>'s reference. Needs the handler
/// installed (the fault reporter) before <see cref="Run"/>. Encodings: Intel SDM volume 2 and AMD APM volume 3/4.
/// </summary>
internal sealed unsafe class AmdInstructionSelfTest
{
    private const ulong DestinationLow = 0x0123_4567_89AB_CDEF;
    private const ulong DestinationHigh = 0xFEDC_BA98_7654_3210;
    private const ulong Bits = 0xA5A5_5A5A_F00D_BEEF;
    private const int Line = AmdInstructionEmulator.CacheLineSize;
    private const int BufferLines = 8;
    private const byte Filled = 0xFF;

    /// <summary>The single CLZERO case clears line 1 from an address inside it; the loop clears lines 1-5 (jb) or walks 4 lines (jbe).</summary>
    private const int ClzeroOffset = Line + 13;
    private const int LoopFirstLine = 1;
    private const int LoopEndLine = 6;
    private const int ClwbLoopBytes = (4 * Line) - 1;
    private const int ClwbLoopPasses = 4;

    /// <summary>CLZERO, the two loops and MONITORX/MWAITX.</summary>
    private const int LineAndMonitorCases = 4;

    /// <summary>The block a SSE4a stub reads and writes at rcx: destination (16 bytes), then source.</summary>
    private const int BlockQuadwords = 4;
    private const int SourceSlot = 2;

    private static readonly byte[] Ud2 = [0x0F, 0x0B];
    private static readonly byte[] Ret = [0xC3];

    /// <summary>movdqu xmm1,[rcx]; movdqu xmm2,[rcx+16] and movdqu [rcx],xmm1; with REX.R (44h) xmm9 and xmm10.</summary>
    private static readonly byte[] LoadXmm = [0xF3, 0x0F, 0x6F, 0x09, 0xF3, 0x0F, 0x6F, 0x51, 0x10];
    private static readonly byte[] LoadXmmExtended = [0xF3, 0x44, 0x0F, 0x6F, 0x09, 0xF3, 0x44, 0x0F, 0x6F, 0x51, 0x10];
    private static readonly byte[] StoreXmm = [0xF3, 0x0F, 0x7F, 0x09];
    private static readonly byte[] StoreXmmExtended = [0xF3, 0x44, 0x0F, 0x7F, 0x09];

    /// <summary>EXTRQ xmm1,imm,imm (66 0F 78 C1); EXTRQ xmm1,xmm2 (66 0F 79 CA); INSERTQ xmm1,xmm2[,imm,imm] (F2 0F 78/79 CA).</summary>
    private static readonly byte[] ExtrqImmediate = [0x66, 0x0F, 0x78, 0xC1];
    private static readonly byte[] ExtrqImmediateExtended = [0x66, 0x41, 0x0F, 0x78, 0xC1];
    private static readonly byte[] ExtrqRegister = [0x66, 0x0F, 0x79, 0xCA];
    private static readonly byte[] ExtrqRegisterExtended = [0x66, 0x45, 0x0F, 0x79, 0xCA];
    private static readonly byte[] InsertqImmediate = [0xF2, 0x0F, 0x78, 0xCA];
    private static readonly byte[] InsertqImmediateExtended = [0xF2, 0x45, 0x0F, 0x78, 0xCA];
    private static readonly byte[] InsertqRegister = [0xF2, 0x0F, 0x79, 0xCA];
    private static readonly byte[] InsertqRegisterExtended = [0xF2, 0x45, 0x0F, 0x79, 0xCA];

    /// <summary>mov rax,rcx; clzero.</summary>
    private static readonly byte[] MovRaxRcx = [0x48, 0x89, 0xC8];
    private static readonly byte[] Clzero = [0x0F, 0x01, 0xFC];

    /// <summary>mov rax,rcx; mov r10,rdx; then clzero; add rax,64; cmp rax,r10; jb back.</summary>
    private static readonly byte[] ClzeroLoopSetup = [0x48, 0x89, 0xC8, 0x49, 0x89, 0xD2];
    private static readonly byte[] ClzeroLoopTail = [0x48, 0x83, 0xC0, 0x40, 0x4C, 0x39, 0xD0, 0x72];

    /// <summary>clwb [rcx]; add rcx,64; cmp rcx,rdx; jbe back.</summary>
    private static readonly byte[] Clwb = [0x66, 0x0F, 0xAE, 0x31];
    private static readonly byte[] ClwbLoopTail = [0x48, 0x83, 0xC1, 0x40, 0x48, 0x39, 0xD1, 0x76];

    /// <summary>push rbx; mov rax,rcx; xor ecx,ecx; xor edx,edx; monitorx; mov ecx,2; mov ebx,1000; xor eax,eax; mwaitx; pop rbx.</summary>
    private static readonly byte[] MonitorSetup = [0x53, 0x48, 0x89, 0xC8, 0x31, 0xC9, 0x31, 0xD2];
    private static readonly byte[] MonitorX = [0x0F, 0x01, 0xFA];
    private static readonly byte[] WaitSetup = [0xB9, 0x02, 0x00, 0x00, 0x00, 0xBB, 0xE8, 0x03, 0x00, 0x00, 0x31, 0xC0];
    private static readonly byte[] MWaitX = [0x0F, 0x01, 0xFB];
    private static readonly byte[] PopRbx = [0x5B];

    /// <summary>The game's forms (INSERTQ immediate, REX registers) and the other three forms of the two instructions.</summary>
    private static readonly Sse4aCase[] Sse4aCases =
    [
        new("INSERTQ xmm9,xmm10,16,16", true, new BitFieldSpec(16, 16), true, Bits, 0),
        new("INSERTQ xmm1,xmm2,8,4", true, new BitFieldSpec(8, 4), false, Bits, 0),
        new("INSERTQ xmm1,xmm2,0(64),0", true, new BitFieldSpec(0, 0), false, Bits, 0),
        new("INSERTQ xmm9,xmm10 (24 at 8)", true, null, true, Bits, 24 | (8 << 8)),
        new("EXTRQ xmm1,8,16", false, new BitFieldSpec(8, 16), false, Bits, 0),
        new("EXTRQ xmm9,12,52", false, new BitFieldSpec(12, 52), true, Bits, 0),
        new("EXTRQ xmm1,xmm2 (12 at 20)", false, null, false, 12 | (20 << 8), 0),
    ];

    private readonly CodeHeap _heap;
    private readonly AmdInstructionHandler _handler;
    private readonly List<byte> _code = [];
    private readonly List<string> _failures = [];

    public AmdInstructionSelfTest(CodeHeap heap, AmdInstructionHandler handler)
    {
        _heap = heap;
        _handler = handler;
    }

    public CpuSelfTestResult Run()
    {
        var sse4a = Sse4aCases.Select(c => (Plain: Add(Sse4aStub(c, false)), Marked: Add(Sse4aStub(c, true)))).ToArray();
        var clzero = (Plain: Add(ClzeroStub(false)), Marked: Add(ClzeroStub(true)));
        var clzeroLoop = (Plain: Add(LoopStub(ClzeroLoopSetup, Clzero, ClzeroLoopTail, false)), Marked: Add(LoopStub(ClzeroLoopSetup, Clzero, ClzeroLoopTail, true)));
        var clwbLoop = (Plain: Add(LoopStub([], Clwb, ClwbLoopTail, false)), Marked: Add(LoopStub([], Clwb, ClwbLoopTail, true)));
        var monitor = (Plain: Add(MonitorStub(false)), Marked: Add(MonitorStub(true)));

        nint code = _heap.Write(CollectionsMarshal.AsSpan(_code));
        _heap.Seal();
        _handler.AllowMarkedCode(code, _code.Count);

        for (int i = 0; i < Sse4aCases.Length; i++)
        {
            CheckSse4a(Sse4aCases[i], code + sse4a[i].Plain, code + sse4a[i].Marked);
        }

        CheckLines("CLZERO", code + clzero.Plain, code + clzero.Marked, ClzeroOffset, 0, LoopFirstLine, LoopFirstLine + 1, 0);
        CheckLines("CLZERO loop", code + clzeroLoop.Plain, code + clzeroLoop.Marked, LoopFirstLine * Line, LoopEndLine * Line, LoopFirstLine, LoopEndLine, LoopEndLine - LoopFirstLine - 1);
        CheckLines("CLWB loop", code + clwbLoop.Plain, code + clwbLoop.Marked, 0, ClwbLoopBytes, 0, 0, ClwbLoopPasses - 1);
        CheckMonitor(code + monitor.Plain, code + monitor.Marked);

        return new CpuSelfTestResult(Sse4aCases.Length + LineAndMonitorCases, _failures.ToArray());
    }

    private static byte[] Sse4aStub(Sse4aCase test, bool marked)
    {
        var stub = new List<byte>(Pick(test, LoadXmm, LoadXmmExtended));
        AddMarker(stub, marked);
        stub.AddRange(Sse4aInstruction(test));

        if (test.Field is BitFieldSpec field)
        {
            stub.Add((byte)field.Length);
            stub.Add((byte)field.Index);
        }

        stub.AddRange(Pick(test, StoreXmm, StoreXmmExtended));
        stub.AddRange(Ret);

        return stub.ToArray();
    }

    private static byte[] Sse4aInstruction(Sse4aCase test)
    {
        bool immediate = test.Field is not null;

        if (test.Insert && immediate)
        {
            return Pick(test, InsertqImmediate, InsertqImmediateExtended);
        }

        if (test.Insert)
        {
            return Pick(test, InsertqRegister, InsertqRegisterExtended);
        }

        if (immediate)
        {
            return Pick(test, ExtrqImmediate, ExtrqImmediateExtended);
        }

        return Pick(test, ExtrqRegister, ExtrqRegisterExtended);
    }

    private static byte[] Pick(Sse4aCase test, byte[] plain, byte[] extended)
    {
        if (test.Extended)
        {
            return extended;
        }

        return plain;
    }

    private static byte[] ClzeroStub(bool marked)
    {
        var stub = new List<byte>(MovRaxRcx);
        AddMarker(stub, marked);
        stub.AddRange(Clzero);
        stub.AddRange(Ret);

        return stub.ToArray();
    }

    /// <summary>setup; loop: [ud2] instruction; tail; rel8 back to the loop's first byte; ret.</summary>
    private static byte[] LoopStub(byte[] setup, byte[] instruction, byte[] tail, bool marked)
    {
        var stub = new List<byte>(setup);
        int loop = stub.Count;
        AddMarker(stub, marked);
        stub.AddRange(instruction);
        stub.AddRange(tail);
        stub.Add((byte)(sbyte)(loop - (stub.Count + 1)));

        stub.AddRange(Ret);

        return stub.ToArray();
    }

    private static byte[] MonitorStub(bool marked)
    {
        var stub = new List<byte>(MonitorSetup);
        AddMarker(stub, marked);
        stub.AddRange(MonitorX);
        stub.AddRange(WaitSetup);
        AddMarker(stub, marked);
        stub.AddRange(MWaitX);

        stub.AddRange(PopRbx);
        stub.AddRange(Ret);

        return stub.ToArray();
    }

    private static void AddMarker(List<byte> stub, bool marked)
    {
        if (marked)
        {
            stub.AddRange(Ud2);
        }
    }

    /// <summary>Appends a stub to the shared code; returns its offset.</summary>
    private int Add(byte[] stub)
    {
        int offset = _code.Count;
        _code.AddRange(stub);

        return offset;
    }

    private void CheckSse4a(Sse4aCase test, nint plain, nint marked)
    {
        var native = RunSse4a(test, plain);
        long before = _handler.Emulated;
        var emulated = RunSse4a(test, marked);
        ulong control = test.Insert ? test.SourceHigh : test.SourceLow;
        var field = test.Field ?? BitField.FromControl(control);
        ulong expected = test.Insert ? BitField.Insert(DestinationLow, test.SourceLow, field) : BitField.Extract(DestinationLow, field);

        if (native != emulated || emulated != (expected, 0UL) || _handler.Emulated == before)
        {
            _failures.Add($"{test.Name}: native {native.High:x16}:{native.Low:x16}, emulated {emulated.High:x16}:{emulated.Low:x16}, reference 0:{expected:x16}");
        }
    }

    private static (ulong Low, ulong High) RunSse4a(Sse4aCase test, nint stub)
    {
        ulong* block = stackalloc ulong[BlockQuadwords];
        block[0] = DestinationLow;
        block[1] = DestinationHigh;
        block[SourceSlot] = test.SourceLow;
        block[SourceSlot + 1] = test.SourceHigh;
        ((delegate* unmanaged<ulong*, void>)stub)(block);

        return (block[0], block[1]);
    }

    /// <summary>
    /// Runs a line stub with rcx and rdx at the given buffer offsets and checks that exactly lines
    /// [<paramref name="firstZero"/>, <paramref name="endZero"/>) were cleared and, for the marked run, that the loop
    /// ran <paramref name="skipped"/> passes inside one fault.
    /// </summary>
    private void CheckLines(string name, nint plain, nint marked, int start, int end, int firstZero, int endZero, long skipped)
    {
        string? native = RunLines(plain, start, end, firstZero, endZero);
        long before = _handler.SkippedPasses;
        string? emulated = RunLines(marked, start, end, firstZero, endZero);
        long ran = _handler.SkippedPasses - before;

        if (native is not null || emulated is not null || ran != skipped)
        {
            _failures.Add($"{name}: native {native ?? "ok"}, emulated {emulated ?? "ok"}, {ran} passes skipped (expected {skipped})");
        }
    }

    private static string? RunLines(nint stub, int start, int end, int firstZero, int endZero)
    {
        byte* buffer = (byte*)NativeMemory.AlignedAlloc(BufferLines * Line, Line);

        try
        {
            new Span<byte>(buffer, BufferLines * Line).Fill(Filled);
            ((delegate* unmanaged<byte*, byte*, void>)stub)(buffer + start, buffer + end);

            for (int line = 0; line < BufferLines; line++)
            {
                byte expected = line >= firstZero && line < endZero ? (byte)0 : Filled;

                if (new Span<byte>(buffer + (line * Line), Line).ContainsAnyExcept(expected))
                {
                    return $"line {line} is not {expected:x2}";
                }
            }

            return null;
        }
        finally
        {
            NativeMemory.AlignedFree(buffer);
        }
    }

    private void CheckMonitor(nint plain, nint marked)
    {
        ulong watched = 0;
        ((delegate* unmanaged<ulong*, void>)plain)(&watched);
        long before = _handler.Count(AmdInstructionKind.MWaitX);
        ((delegate* unmanaged<ulong*, void>)marked)(&watched);

        if (_handler.Count(AmdInstructionKind.MWaitX) == before)
        {
            _failures.Add("MONITORX/MWAITX: the marked MWAITX was not emulated");
        }
    }
}
