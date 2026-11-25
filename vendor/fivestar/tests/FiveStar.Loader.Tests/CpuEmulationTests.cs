// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Cpu;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The emulation of the Zen 2 instructions an Intel CPU lacks: the decoder on the game's encodings, the emulation on
/// hand-made CONTEXTs, and the self-test through a real vectored handler, which on an AMD CPU compares every case with
/// the hardware.
/// </summary>
public sealed unsafe class CpuEmulationTests
{
    // CONTEXT (winnt.h, AMD64): size, ContextFlags with CONTEXT_FULL | CONTEXT_FLOATING_POINT, Rax, R10, Rip, Xmm0.
    private const int ContextSize = 0x4D0;
    private const int ContextFlags = 0x30;
    private const uint ContextFull = 0x0010000B;
    private const int Rax = 0x78;
    private const int R9 = 0x78 + (9 * 8);
    private const int R10 = 0x78 + (10 * 8);
    private const int Rip = 0xF8;
    private const int Xmm0 = 0x1A0;
    private const uint IllegalInstruction = 0xC000001D;
    private const uint AccessViolation = 0xC0000005;

    private static AmdInstructionHandler? _installed;

    [Theory]
    [InlineData(new byte[] { 0xF2, 0x44, 0x0F, 0x78, 0xCF, 0x08, 0x00 }, (int)AmdInstructionKind.Insertq, 7)]
    [InlineData(new byte[] { 0xF2, 0x0F, 0x79, 0xCA }, (int)AmdInstructionKind.Insertq, 4)]
    [InlineData(new byte[] { 0x66, 0x41, 0x0F, 0x78, 0xC1, 0x05, 0x3B }, (int)AmdInstructionKind.Extrq, 7)]
    [InlineData(new byte[] { 0x0F, 0x01, 0xFC }, (int)AmdInstructionKind.Clzero, 3)]
    [InlineData(new byte[] { 0x67, 0x0F, 0x01, 0xFC }, (int)AmdInstructionKind.Clzero, 4)]
    [InlineData(new byte[] { 0x0F, 0x01, 0xFA }, (int)AmdInstructionKind.MonitorX, 3)]
    [InlineData(new byte[] { 0x0F, 0x01, 0xFB }, (int)AmdInstructionKind.MWaitX, 3)]
    [InlineData(new byte[] { 0x66, 0x41, 0x0F, 0xAE, 0x31 }, (int)AmdInstructionKind.Clwb, 5)]
    [InlineData(new byte[] { 0x66, 0x0F, 0xAE, 0x74, 0x24, 0x08 }, (int)AmdInstructionKind.Clwb, 6)]
    [InlineData(new byte[] { 0x66, 0x0F, 0xAE, 0x35, 0x10, 0x20, 0x30, 0x40 }, (int)AmdInstructionKind.Clwb, 8)]
    [InlineData(new byte[] { 0x66, 0x0F, 0xAE, 0xB0, 0x10, 0x20, 0x30, 0x40 }, (int)AmdInstructionKind.Clwb, 8)]
    public void Decoder_reads_the_encodings_with_their_lengths(byte[] code, int kind, int length)
    {
        Assert.True(AmdInstructionDecoder.TryDecode(Padded(code), out var instruction));
        Assert.Equal((AmdInstructionKind)kind, instruction.Kind);
        Assert.Equal(length, instruction.Length);
    }

    [Theory]
    [InlineData(new byte[] { 0x0F, 0x0B })]
    [InlineData(new byte[] { 0xF0, 0x0F, 0x01, 0xFC })]
    [InlineData(new byte[] { 0x66, 0x0F, 0x01, 0xFC })]
    [InlineData(new byte[] { 0x66, 0x0F, 0xAE, 0xF1 })]
    [InlineData(new byte[] { 0x0F, 0xAE, 0x31 })]
    [InlineData(new byte[] { 0x66, 0x0F, 0x78, 0xC9, 0x08, 0x00 })]
    [InlineData(new byte[] { 0x66, 0xF2, 0x0F, 0x78, 0xC1, 0x08, 0x00 })]
    [InlineData(new byte[] { 0xF2, 0x0F, 0x78, 0x01, 0x08, 0x00 })]
    public void Decoder_rejects_every_other_encoding(byte[] code)
    {
        Assert.False(AmdInstructionDecoder.TryDecode(Padded(code), out _));
    }

    [Fact]
    public void Bit_fields_follow_the_manual()
    {
        const ulong value = 0x0123_4567_89AB_CDEF;

        Assert.Equal(0xABUL, BitField.Extract(value, new BitFieldSpec(8, 16)));
        Assert.Equal(value, BitField.Extract(value, new BitFieldSpec(0, 0)));
        Assert.Equal(0x0123_4567_BEEF_CDEFUL, BitField.Insert(value, 0xF00D_BEEF, new BitFieldSpec(16, 16)));
        Assert.Equal(0xF123_4567_89AB_CDEFUL, BitField.Insert(value, 0xFF, new BitFieldSpec(4, 60)));
        Assert.Equal(new BitFieldSpec(24, 8), BitField.FromControl(0x0818));
    }

    [Fact]
    public void Insertq_on_a_context_sets_the_low_quadword_clears_the_high_one_and_steps_over_it()
    {
        // insertq xmm9,xmm7,8,0: the game's form with a REX.R destination.
        using var frame = new Frame([0xF2, 0x44, 0x0F, 0x78, 0xCF, 0x08, 0x00], IllegalInstruction);
        Xmm(frame.Context, 9)[0] = 0x1111_2222_3333_4444;
        Xmm(frame.Context, 9)[1] = 0x5555_6666_7777_8888;
        Xmm(frame.Context, 7)[0] = 0xAB;

        Assert.True(Handler().TryEmulate(frame.Pointers));
        Assert.Equal(0x1111_2222_3333_44ABUL, Xmm(frame.Context, 9)[0]);
        Assert.Equal(0UL, Xmm(frame.Context, 9)[1]);
        Assert.Equal(frame.Code + 7, *(nint*)(frame.Context + Rip));
    }

    [Fact]
    public void Clzero_clears_the_aligned_line_and_loops_run_in_two_faults()
    {
        // clzero; add rax,64; cmp rax,r10; jb back: the game's loop shape.
        using var frame = new Frame([0x0F, 0x01, 0xFC, 0x48, 0x83, 0xC0, 0x40, 0x4C, 0x39, 0xD0, 0x72, 0xF4], IllegalInstruction);
        byte* buffer = (byte*)NativeMemory.AlignedAlloc(512, 64);
        new Span<byte>(buffer, 512).Fill(0xFF);
        *(ulong*)(frame.Context + Rax) = (ulong)(buffer + 64 + 7);
        *(ulong*)(frame.Context + R10) = (ulong)(buffer + 320);
        var handler = Handler();

        Assert.True(handler.TryEmulate(frame.Pointers));

        // Lines 1-3 ran inside the fault; RIP stays on the clzero for the last pass.
        Assert.Equal(3, handler.SkippedPasses);
        Assert.Equal((ulong)(buffer + (4 * 64) + 7), *(ulong*)(frame.Context + Rax));
        Assert.Equal(frame.Code, *(nint*)(frame.Context + Rip));

        Assert.True(handler.TryEmulate(frame.Pointers));
        Assert.Equal(frame.Code + 3, *(nint*)(frame.Context + Rip));
        Assert.False(new Span<byte>(buffer + 64, 256).ContainsAnyExcept((byte)0));
        Assert.False(new Span<byte>(buffer, 64).ContainsAnyExcept((byte)0xFF));
        Assert.False(new Span<byte>(buffer + 320, 192).ContainsAnyExcept((byte)0xFF));
        NativeMemory.AlignedFree(buffer);
    }

    [Fact]
    public void The_rex_b_clwb_loop_of_the_game_runs_in_two_faults()
    {
        // clwb [r9]; add r9,64; cmp r9,rax; jbe back: the bytes at game RVA 0x4ecc84a (issue #13).
        using var frame = new Frame([0x66, 0x41, 0x0F, 0xAE, 0x31, 0x49, 0x83, 0xC1, 0x40, 0x49, 0x39, 0xC1, 0x76, 0xF2], IllegalInstruction);
        const ulong start = 0x10000;
        *(ulong*)(frame.Context + R9) = start;
        *(ulong*)(frame.Context + Rax) = start + 255;
        var handler = Handler();

        Assert.True(handler.TryEmulate(frame.Pointers));
        Assert.Equal(3, handler.SkippedPasses);
        Assert.Equal(start + (3 * 64), *(ulong*)(frame.Context + R9));
        Assert.Equal(frame.Code, *(nint*)(frame.Context + Rip));

        Assert.True(handler.TryEmulate(frame.Pointers));
        Assert.Equal(frame.Code + 5, *(nint*)(frame.Context + Rip));
        Assert.Equal(2, handler.Count(AmdInstructionKind.Clwb));
    }

    [Fact]
    public void Other_exceptions_and_instructions_are_left_alone()
    {
        using var ud2 = new Frame([0x0F, 0x0B], IllegalInstruction);
        using var access = new Frame([0x0F, 0x01, 0xFC], AccessViolation);

        Assert.False(Handler().TryEmulate(ud2.Pointers));
        Assert.False(Handler().TryEmulate(access.Pointers));
        Assert.Equal(access.Code, *(nint*)(access.Context + Rip));
    }

    /// <summary>On an AMD CPU this also checks every case against the hardware; on others it runs the real emulation path.</summary>
    [Fact]
    public void Self_test_passes_through_a_vectored_handler()
    {
        var handler = Handler();
        _installed = handler;
        delegate* unmanaged<nint, int> entry = &OnException;
        nint registration = Kernel32.AddVectoredExceptionHandler(1, (nint)entry);

        try
        {
            var result = new AmdInstructionSelfTest(new CodeHeap(), handler).Run();

            Assert.True(result.Failures.Count == 0, string.Join(Environment.NewLine, result.Failures));
            Assert.True(result.Cases > 0);
        }
        finally
        {
            _ = Kernel32.RemoveVectoredExceptionHandler(registration);
            _installed = null;
        }
    }

    [UnmanagedCallersOnly]
    private static int OnException(nint pointers)
    {
        var handler = _installed;

        if (handler is not null && handler.TryEmulate(pointers))
        {
            return -1;
        }

        return 0;
    }

    private static AmdInstructionHandler Handler()
    {
        return new AmdInstructionHandler(new ImageTable(), new LoaderLog(TextWriter.Null, null, false));
    }

    private static ReadOnlySpan<byte> Padded(byte[] code)
    {
        var padded = new byte[AmdInstructionDecoder.MaxLength];
        code.CopyTo(padded, 0);

        return padded;
    }

    private static ulong* Xmm(byte* context, int number)
    {
        return (ulong*)(context + Xmm0 + (number * 16));
    }

    /// <summary>A faulting thread's EXCEPTION_POINTERS with RIP on a copy of the given code.</summary>
    private sealed class Frame : IDisposable
    {
        public Frame(byte[] code, uint status)
        {
            Code = (nint)NativeMemory.AllocZeroed(64);
            code.CopyTo(new Span<byte>((void*)Code, code.Length));
            Context = (byte*)NativeMemory.AlignedAlloc(ContextSize, 16);
            new Span<byte>(Context, ContextSize).Clear();
            *(uint*)(Context + ContextFlags) = ContextFull;
            *(nint*)(Context + Rip) = Code;

            Record = (byte*)NativeMemory.AllocZeroed(0x98);
            *(uint*)Record = status;
            Pointers = (nint)NativeMemory.Alloc(16);
            *(byte**)Pointers = Record;
            *(byte**)(Pointers + 8) = Context;
        }

        public nint Code { get; }

        public byte* Context { get; }

        public byte* Record { get; }

        public nint Pointers { get; }

        public void Dispose()
        {
            NativeMemory.Free((void*)Code);
            NativeMemory.AlignedFree(Context);
            NativeMemory.Free(Record);
            NativeMemory.Free((void*)Pointers);
        }
    }
}
