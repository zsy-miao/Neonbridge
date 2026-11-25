// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// The native entry of the fault reporter's vectored handler. The handler is an [UnmanagedCallersOnly] method, and
/// entering one on a thread that runs managed code (an access violation in our own C#, or in the runtime's helpers)
/// ends the process at once: "Invalid Program: attempted to call a UnmanagedCallersOnly method from managed code",
/// without a stack. Such faults are the runtime's own to turn into exceptions, so the entry passes them on and calls
/// the handler only for a fault in code that is not managed:
/// <list type="bullet">
/// <item>in a loaded module (RtlPcToFileHeader finds its base) other than coreclr.dll and clrjit.dll and not a managed
/// image (no COM descriptor in its PE header);</item>
/// <item>or, outside every module (where the runtime's JIT code also lives), in an image this loader mapped (the table
/// at <c>mappedRanges</c>: a count, then base and end pairs).</item>
/// </list>
/// Any other fault returns EXCEPTION_CONTINUE_SEARCH. The thread's TLS array is checked first, as before.
/// </summary>
internal static class FaultEntryCode
{
    // EXCEPTION_POINTERS.ExceptionRecord in rcx; EXCEPTION_RECORD.ExceptionAddress at +0x10 (winnt.h).
    private static readonly byte[] Prologue =
    [
        0x53,                               // push rbx
        0x48, 0x83, 0xEC, 0x30,             // sub rsp,30h
        0x48, 0x89, 0xCB,                   // mov rbx,rcx
        0x48, 0x8B, 0x03,                   // mov rax,[rbx]
        0x48, 0x8B, 0x48, 0x10,             // mov rcx,[rax+10h]
        0x48, 0x89, 0x4C, 0x24, 0x28,       // mov [rsp+28h],rcx
        0x48, 0x8D, 0x54, 0x24, 0x20,       // lea rdx,[rsp+20h]
    ];

    private static readonly byte[] MovRax = [0x48, 0xB8];
    private static readonly byte[] MovR11 = [0x49, 0xBB];
    private static readonly byte[] CallRax = [0xFF, 0xD0];
    private static readonly byte[] TestRax = [0x48, 0x85, 0xC0];
    private static readonly byte[] CmpRaxR11 = [0x4C, 0x39, 0xD8];
    private static readonly byte[] Jz = [0x0F, 0x84];
    private static readonly byte[] Jnz = [0x0F, 0x85];
    private static readonly byte[] Jmp = [0xE9];

    // The module's PE header: e_lfanew at +3Ch; the COM descriptor's RVA (data directory 14) at e_lfanew + 0F8h (PE32+).
    private static readonly byte[] ComDescriptor =
    [
        0x8B, 0x48, 0x3C,                           // mov ecx,[rax+3Ch]
        0x8B, 0x8C, 0x08, 0xF8, 0x00, 0x00, 0x00,   // mov ecx,[rax+rcx+0F8h]
        0x85, 0xC9,                                 // test ecx,ecx
    ];

    private static readonly byte[] LoadAddressAndCount =
    [
        0x48, 0x8B, 0x4C, 0x24, 0x28,   // mov rcx,[rsp+28h]
    ];

    private static readonly byte[] CountAndFirst =
    [
        0x45, 0x8B, 0x13,               // mov r10d,[r11]
        0x49, 0x83, 0xC3, 0x08,         // add r11,8
    ];

    private static readonly byte[] TestCount = [0x45, 0x85, 0xD2];      // test r10d,r10d
    private static readonly byte[] InRange =
    [
        0x49, 0x3B, 0x0B,               // cmp rcx,[r11]
        0x72, 0x0B,                     // jb next (past the cmp, the jae and the jmp to the handler)
        0x49, 0x3B, 0x4B, 0x08,         // cmp rcx,[r11+8]
        0x73, 0x05,                     // jae next (past the jmp to the handler)
    ];

    private static readonly byte[] Next =
    [
        0x49, 0x83, 0xC3, 0x10,         // add r11,16
        0x41, 0xFF, 0xCA,               // dec r10d
    ];

    private static readonly byte[] Enter =
    [
        0x48, 0x89, 0xD9,               // mov rcx,rbx
        0x48, 0x83, 0xC4, 0x30,         // add rsp,30h
        0x5B,                           // pop rbx
    ];

    private static readonly byte[] JmpRax = [0xFF, 0xE0];

    private static readonly byte[] Skip =
    [
        0x48, 0x83, 0xC4, 0x30,         // add rsp,30h
        0x5B,                           // pop rbx
        0x31, 0xC0,                     // xor eax,eax (EXCEPTION_CONTINUE_SEARCH)
        0xC3,                           // ret
    ];

    /// <summary>A rel8 jump back to the range loop's count test: <c>jmp</c> and its displacement.</summary>
    private const byte JmpRel8 = 0xEB;

    /// <summary>Bytes of a rel32 displacement.</summary>
    private const int Rel32 = 4;

    /// <summary>
    /// The code after the TLS-array test: <paramref name="handler"/> is the managed handler's entry,
    /// <paramref name="pcToModule"/> ntdll's RtlPcToFileHeader, <paramref name="coreclr"/> and <paramref name="clrjit"/>
    /// the runtime's module bases (0 for none) and <paramref name="mappedRanges"/> the mapped images' table.
    /// </summary>
    public static byte[] Body(nint handler, nint pcToModule, nint coreclr, nint clrjit, nint mappedRanges)
    {
        var code = new List<byte>();
        code.AddRange(Prologue);
        Immediate(code, MovRax, pcToModule);
        code.AddRange(CallRax);
        code.AddRange(TestRax);
        int toNoModule = Jump(code, Jz);

        Immediate(code, MovR11, coreclr);
        code.AddRange(CmpRaxR11);
        int coreclrSkip = Jump(code, Jz);
        Immediate(code, MovR11, clrjit);
        code.AddRange(CmpRaxR11);
        int clrjitSkip = Jump(code, Jz);

        code.AddRange(ComDescriptor);
        int managedSkip = Jump(code, Jnz);
        int moduleEnter = Jump(code, Jmp);

        Land(code, toNoModule);
        code.AddRange(LoadAddressAndCount);
        Immediate(code, MovR11, mappedRanges);
        code.AddRange(CountAndFirst);

        int loop = code.Count;
        code.AddRange(TestCount);
        int endSkip = Jump(code, Jz);
        code.AddRange(InRange);
        int rangeEnter = Jump(code, Jmp);

        code.AddRange(Next);
        code.Add(JmpRel8);
        code.Add((byte)(loop - (code.Count + 1)));

        Land(code, moduleEnter);
        Land(code, rangeEnter);
        code.AddRange(Enter);
        Immediate(code, MovRax, handler);
        code.AddRange(JmpRax);

        Land(code, coreclrSkip);
        Land(code, clrjitSkip);
        Land(code, managedSkip);
        Land(code, endSkip);
        code.AddRange(Skip);

        return [.. code];
    }

    private static void Immediate(List<byte> code, byte[] opcode, nint value)
    {
        code.AddRange(opcode);
        Span<byte> bytes = stackalloc byte[sizeof(ulong)];
        BinaryPrimitives.WriteUInt64LittleEndian(bytes, (ulong)value);
        code.AddRange(bytes.ToArray());
    }

    /// <summary>A jump with a rel32 displacement to patch; returns the displacement's position.</summary>
    private static int Jump(List<byte> code, byte[] opcode)
    {
        code.AddRange(opcode);
        int at = code.Count;
        code.AddRange(new byte[Rel32]);

        return at;
    }

    /// <summary>Points the jump whose displacement is at <paramref name="at"/> to the current position.</summary>
    private static void Land(List<byte> code, int at)
    {
        int displacement = code.Count - (at + Rel32);
        Span<byte> bytes = stackalloc byte[Rel32];
        BinaryPrimitives.WriteInt32LittleEndian(bytes, displacement);

        for (int i = 0; i < Rel32; i++)
        {
            code[at + i] = bytes[i];
        }
    }
}
