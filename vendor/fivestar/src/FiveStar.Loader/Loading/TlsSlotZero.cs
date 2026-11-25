// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Codegen;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Loading;

/// <summary>
/// TLS slot 0 for a mapped executable. The linker compiles an executable's thread_local accesses as
/// <c>mov rax,gs:[58h]; mov rax,[rax]</c>: the executable's slot is always 0, so it never reads _tls_index. In this
/// process slot 0 belongs to the host executable, so each thread's slot-0 pointer is pointed at the mapped
/// executable's block (the TLS host's slot) while the thread lives, and put back before ntdll frees the blocks. The
/// original pointer is kept in 8 bytes the host template adds after the executable's TLS data.
/// Two TLS callbacks of the host do this: <see cref="EmitEnter"/> runs first (process and thread attach),
/// <see cref="EmitLeave"/> last (process and thread detach). Encodings: Intel SDM volume 2.
/// </summary>
internal static class TlsSlotZero
{
    /// <summary>Bytes the host template adds for the saved slot-0 pointer.</summary>
    public const int SaveSize = sizeof(ulong);

    private const byte JaRel8 = 0x77;
    private const byte JeRel8 = 0x74;
    private const byte JneRel8 = 0x75;
    private const byte Ret = 0xC3;

    /// <summary>DLL_PROCESS_DETACH, DLL_PROCESS_ATTACH, DLL_THREAD_ATTACH, DLL_THREAD_DETACH (winnt.h).</summary>
    private const byte ThreadDetach = 3;

    /// <summary><c>lea eax,[rdx-1]; cmp eax,1</c>: reason 1 or 2 leaves eax &lt;= 1 (unsigned).</summary>
    private static readonly byte[] IsAttach = [0x8D, 0x42, 0xFF, 0x83, 0xF8, 0x01];

    /// <summary><c>mov r10,gs:[58h]</c>.</summary>
    private static readonly byte[] LoadTlsArray = [0x65, 0x4C, 0x8B, 0x14, 0x25, ThreadEnvironmentBlock.ThreadLocalStoragePointer, 0, 0, 0];

    /// <summary><c>mov r11,[r10+rax*8]</c>: the executable's block.</summary>
    private static readonly byte[] LoadBlock = [0x4D, 0x8B, 0x1C, 0xC2];

    /// <summary><c>mov rax,[r10]; cmp rax,r11</c>.</summary>
    private static readonly byte[] LoadSlotZeroCompare = [0x49, 0x8B, 0x02, 0x4C, 0x39, 0xD8];

    /// <summary><c>mov [r11+disp32],rax</c>.</summary>
    private static readonly byte[] SaveRax = [0x49, 0x89, 0x83];

    /// <summary><c>mov [r10],r11</c>.</summary>
    private static readonly byte[] SlotZeroFromBlock = [0x4D, 0x89, 0x1A];

    /// <summary><c>test edx,edx</c>.</summary>
    private static readonly byte[] TestReason = [0x85, 0xD2];

    /// <summary><c>cmp edx,imm8</c>.</summary>
    private static readonly byte[] CompareReason = [0x83, 0xFA];

    /// <summary><c>mov rax,[r11+disp32]</c>.</summary>
    private static readonly byte[] LoadSaved = [0x49, 0x8B, 0x83];

    /// <summary><c>test rax,rax</c>.</summary>
    private static readonly byte[] TestRax = [0x48, 0x85, 0xC0];

    /// <summary><c>mov [r10],rax</c>.</summary>
    private static readonly byte[] SlotZeroFromRax = [0x49, 0x89, 0x02];

    /// <summary><c>mov qword [r11+disp32],imm32</c>.</summary>
    private static readonly byte[] ClearSaved = [0x49, 0xC7, 0x83];

    /// <summary>Offset of the saved pointer in the block: after the executable's data, 8-byte aligned.</summary>
    public static int SaveOffset(TlsTemplate template)
    {
        long size = template.RawData.Length + template.ZeroFill;

        return (int)((size + SaveSize - 1) & ~(long)(SaveSize - 1));
    }

    /// <summary>On attach: save slot 0 and point it at the executable's block (unless it already is).</summary>
    public static void EmitEnter(X64Emitter code, uint textRva, uint hostIndexRva, int saveOffset)
    {
        code.Data(IsAttach);
        int toDone = Jump(code, JaRel8);
        LoadBlockPointer(code, textRva, hostIndexRva);
        code.Data(LoadSlotZeroCompare);
        int toDoneSame = Jump(code, JeRel8);

        code.Data(SaveRax);
        code.Data32((uint)saveOffset);
        code.Data(SlotZeroFromBlock);
        code.PatchJumpHere(toDone);
        code.PatchJumpHere(toDoneSame);
        code.Data([Ret]);
    }

    /// <summary>On detach: put the saved slot-0 pointer back.</summary>
    public static void EmitLeave(X64Emitter code, uint textRva, uint hostIndexRva, int saveOffset)
    {
        code.Data(TestReason);
        int toRestore = Jump(code, JeRel8);
        code.Data(CompareReason);
        code.Data([ThreadDetach]);
        int toDone = Jump(code, JneRel8);
        code.PatchJumpHere(toRestore);

        LoadBlockPointer(code, textRva, hostIndexRva);
        code.Data(LoadSaved);
        code.Data32((uint)saveOffset);
        code.Data(TestRax);
        int toDoneEmpty = Jump(code, JeRel8);

        code.Data(SlotZeroFromRax);
        code.Data(ClearSaved);
        code.Data32((uint)saveOffset);
        code.Data32(0);

        code.PatchJumpHere(toDone);
        code.PatchJumpHere(toDoneEmpty);
        code.Data([Ret]);
    }

    /// <summary>eax = host _tls_index, r10 = the TLS array, r11 = the executable's block.</summary>
    private static void LoadBlockPointer(X64Emitter code, uint textRva, uint hostIndexRva)
    {
        uint next = textRva + (uint)code.Length + X64Emitter.MovEaxRipRelativeLength;
        code.LoadEaxRipRelative((int)(hostIndexRva - next));
        code.Data(LoadTlsArray);
        code.Data(LoadBlock);
    }

    private static int Jump(X64Emitter code, byte opcode)
    {
        code.Data([opcode, 0]);

        return code.Length - 1;
    }
}
