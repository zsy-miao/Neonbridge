// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Codegen;

/// <summary>
/// Appends the few x64 instructions the loader generates. Each encoding is the one given in the Intel 64 and IA-32
/// Architectures Software Developer's Manual, volume 2, for the instruction named in its field.
/// </summary>
internal sealed class X64Emitter
{
    /// <summary>Length of <c>mov eax,[rip+disp32]</c>; its displacement is relative to the end of the instruction.</summary>
    public const int MovEaxRipRelativeLength = 6;

    /// <summary>Register numbers in ModRM order (rdx 2, r8 8, r9 9).</summary>
    public const int Rdx = 2;
    public const int R8 = 8;
    public const int R9 = 9;

    private const int R10 = 10;
    private const int Rsp = 4;
    private const int Rbp = 5;
    private const int FirstExtendedRegister = 8;
    private const int LowRegisterMask = 7;
    private const int ModRmRegShift = 3;

    /// <summary>ModRM mod field values: register direct, memory with no displacement, memory with an 8-bit one.</summary>
    private const byte ModRegister = 0xC0;
    private const byte ModMemory = 0x00;
    private const byte ModMemoryDisp8 = 0x40;

    /// <summary>REX.W, and the REX.R and REX.B bits that extend the ModRM reg and r/m fields to r8-r15.</summary>
    private const byte RexW = 0x48;
    private const byte RexR = 0x04;
    private const byte RexB = 0x01;

    private const byte JaeRel8Opcode = 0x73;
    private const byte TestOpcode = 0x85;
    private const byte MovLoadOpcode = 0x8B;
    private const byte RetOpcode = 0xC3;
    private const byte Int3Opcode = 0xCC;
    private const byte JzRel8Opcode = 0x74;
    private const byte MovEaxImm32Opcode = 0xB8;

    private static readonly byte[] SubRspImm8 = [0x48, 0x83, 0xEC];
    private static readonly byte[] AddRspImm8 = [0x48, 0x83, 0xC4];
    private static readonly byte[] MovRaxImm64 = [0x48, 0xB8];
    private static readonly byte[] MovRcxImm64 = [0x48, 0xB9];
    private static readonly byte[] IncDwordRcxDisp32 = [0xFF, 0x81];
    private static readonly byte[] MovR11Imm64 = [0x49, 0xBB];
    private static readonly byte[] CallRaxEncoding = [0xFF, 0xD0];
    private static readonly byte[] JmpRaxEncoding = [0xFF, 0xE0];
    private static readonly byte[] MovRspDisp8FromRax = [0x48, 0x89, 0x44, 0x24];
    private static readonly byte[] MovRaxFromRspDisp8 = [0x48, 0x8B, 0x44, 0x24];
    private static readonly byte[] MovRaxFromRspDisp32 = [0x48, 0x8B, 0x84, 0x24];
    private static readonly byte[] MovR9FromRspDisp32 = [0x4C, 0x8B, 0x8C, 0x24];
    private static readonly byte[] MovRdxFromRspEncoding = [0x48, 0x8B, 0x14, 0x24];
    private static readonly byte[] MovRcxFromGsDisp32 = [0x65, 0x48, 0x8B, 0x0C, 0x25];
    private static readonly byte[] MovGsDisp32FromEcx = [0x65, 0x89, 0x0C, 0x25];
    private static readonly byte[] MovGsDisp32QwordImm32 = [0x65, 0x48, 0xC7, 0x04, 0x25];
    private static readonly byte[] TestRcxRcxEncoding = [0x48, 0x85, 0xC9];
    private static readonly byte[] DecRcxEncoding = [0x48, 0xFF, 0xC9];
    private static readonly byte[] MovRcxRdxEncoding = [0x48, 0x89, 0xD1];
    private static readonly byte[] MovRdxR8Encoding = [0x4C, 0x89, 0xC2];
    private static readonly byte[] MovR8R9Encoding = [0x4D, 0x89, 0xC8];
    private static readonly byte[] MovapsXmm0Xmm1 = [0x0F, 0x28, 0xC1];
    private static readonly byte[] MovapsXmm1Xmm2 = [0x0F, 0x28, 0xCA];
    private static readonly byte[] MovapsXmm2Xmm3 = [0x0F, 0x28, 0xD3];
    private static readonly byte[] MovqXmm3FromRspDisp32 = [0xF3, 0x0F, 0x7E, 0x9C, 0x24];
    private static readonly byte[] MovEaxFromRipDisp32 = [0x8B, 0x05];
    private static readonly byte[] MovR11MemFromEax = [0x41, 0x89, 0x03];
    private static readonly byte[] MovRcxFromRcxDisp8 = [0x48, 0x8B, 0x49];
    private static readonly byte[] MovRdxFromRdxDisp8 = [0x48, 0x8B, 0x52];
    private static readonly byte[] MovRaxFromRcxEncoding = [0x48, 0x8B, 0x01];
    private static readonly byte[] MovRaxFromRdxEncoding = [0x48, 0x8B, 0x02];
    private static readonly byte[] XchgRcxRdxEncoding = [0x48, 0x87, 0xCA];
    private static readonly byte[] JmpRaxDisp32Encoding = [0xFF, 0xA0];
    private static readonly byte[] MovR10Imm64 = [0x49, 0xBA];
    private static readonly byte[] MovR11dImm32 = [0x41, 0xBB];
    private static readonly byte[] LockXaddR10MemR11 = [0xF0, 0x4D, 0x0F, 0xC1, 0x1A];
    private static readonly byte[] LockIncQwordR10Mem = [0xF0, 0x49, 0xFF, 0x02];
    private static readonly byte[] AndR11dImm32 = [0x41, 0x81, 0xE3];
    private static readonly byte[] MovDwordR10R11x4Disp8Imm32 = [0x43, 0xC7, 0x44, 0x9A];
    private static readonly byte[] JmpR10Encoding = [0x41, 0xFF, 0xE2];
    private static readonly byte[] SubR10R11Encoding = [0x4D, 0x29, 0xDA];
    private static readonly byte[] CmpR10Imm32Encoding = [0x49, 0x81, 0xFA];

    private readonly List<byte> _code = [];

    /// <summary>Bytes emitted so far.</summary>
    public int Length => _code.Count;

    public byte[] ToArray()
    {
        return _code.ToArray();
    }

    public void SubRsp(byte size)
    {
        Emit(SubRspImm8, size);
    }

    public void AddRsp(byte size)
    {
        Emit(AddRspImm8, size);
    }

    public void MovRax(ulong value)
    {
        Emit(MovRaxImm64);
        Emit64(value);
    }

    public void MovRcx(ulong value)
    {
        Emit(MovRcxImm64);
        Emit64(value);
    }

    public void MovR11(ulong value)
    {
        Emit(MovR11Imm64);
        Emit64(value);
    }

    public void MovR10(ulong value)
    {
        Emit(MovR10Imm64);
        Emit64(value);
    }

    /// <summary><c>mov r11d,imm32</c> (zero-extends into r11).</summary>
    public void MovR11d(uint value)
    {
        Emit(MovR11dImm32);
        Emit32(value);
    }

    /// <summary><c>lock xadd [r10],r11</c>.</summary>
    public void LockXaddR10R11()
    {
        Emit(LockXaddR10MemR11);
    }

    /// <summary><c>inc dword [rcx+disp32]</c> (flags change; no argument register does).</summary>
    public void IncDwordAtRcx(int displacement)
    {
        Emit(IncDwordRcxDisp32);
        Emit32((uint)displacement);
    }

    /// <summary><c>lock inc qword [r10]</c>.</summary>
    public void LockIncR10()
    {
        Emit(LockIncQwordR10Mem);
    }

    /// <summary><c>and r11d,imm32</c>.</summary>
    public void AndR11d(uint value)
    {
        Emit(AndR11dImm32);
        Emit32(value);
    }

    /// <summary><c>mov dword [r10+r11*4+disp8],imm32</c>.</summary>
    public void StoreDwordR10IndexR11(byte displacement, uint value)
    {
        Emit(MovDwordR10R11x4Disp8Imm32, displacement);
        Emit32(value);
    }

    public void JmpR10()
    {
        Emit(JmpR10Encoding);
    }

    public void MovEax(uint value)
    {
        _code.Add(MovEaxImm32Opcode);
        Emit32(value);
    }

    public void CallRax()
    {
        Emit(CallRaxEncoding);
    }

    public void JmpRax()
    {
        Emit(JmpRaxEncoding);
    }

    public void Ret()
    {
        _code.Add(RetOpcode);
    }

    /// <summary><c>mov [rsp+disp8],rax</c>.</summary>
    public void StoreRaxToStack(byte displacement)
    {
        Emit(MovRspDisp8FromRax, displacement);
    }

    /// <summary><c>mov rax,[rsp+disp8]</c>.</summary>
    public void LoadRaxFromStack(byte displacement)
    {
        Emit(MovRaxFromRspDisp8, displacement);
    }

    /// <summary><c>mov rdx,[rsp]</c>: at a function's first instruction, its return address.</summary>
    public void LoadRdxFromStackTop()
    {
        Emit(MovRdxFromRspEncoding);
    }

    /// <summary><c>mov rax,[rsp+disp32]</c>.</summary>
    public void LoadRaxFromStackFar(int displacement)
    {
        Emit(MovRaxFromRspDisp32);
        Emit32((uint)displacement);
    }

    /// <summary><c>mov r9,[rsp+disp32]</c>.</summary>
    public void LoadR9FromStack(int displacement)
    {
        Emit(MovR9FromRspDisp32);
        Emit32((uint)displacement);
    }

    /// <summary><c>movq xmm3,[rsp+disp32]</c>.</summary>
    public void LoadXmm3FromStack(int displacement)
    {
        Emit(MovqXmm3FromRspDisp32);
        Emit32((uint)displacement);
    }

    /// <summary><c>mov rcx,gs:[disp32]</c>.</summary>
    public void LoadRcxFromGs(int displacement)
    {
        Emit(MovRcxFromGsDisp32);
        Emit32((uint)displacement);
    }

    /// <summary><c>mov gs:[disp32],ecx</c>.</summary>
    public void StoreEcxToGs(int displacement)
    {
        Emit(MovGsDisp32FromEcx);
        Emit32((uint)displacement);
    }

    /// <summary><c>mov qword gs:[disp32],imm32</c> (sign-extended).</summary>
    public void StoreQwordToGs(int displacement, int value)
    {
        Emit(MovGsDisp32QwordImm32);
        Emit32((uint)displacement);
        Emit32((uint)value);
    }

    public void TestRcxRcx()
    {
        Emit(TestRcxRcxEncoding);
    }

    public void DecRcx()
    {
        Emit(DecRcxEncoding);
    }

    /// <summary><c>test reg,reg</c> (64-bit) for <see cref="Rdx"/>, <see cref="R8"/> or <see cref="R9"/>.</summary>
    public void TestRegister(int register)
    {
        _code.Add(Rex(register, register));
        _code.Add(TestOpcode);
        _code.Add(ModRm(ModRegister, register, register));
    }

    /// <summary><c>mov r10,[reg]</c>.</summary>
    public void LoadR10FromRegister(int register)
    {
        CheckPlainBase(register);
        _code.Add(Rex(R10, register));
        _code.Add(MovLoadOpcode);
        _code.Add(ModRm(ModMemory, R10, register));
    }

    /// <summary><c>mov reg,[reg+disp8]</c>.</summary>
    public void LoadRegisterFromItself(int register, byte displacement)
    {
        CheckPlainBase(register);
        _code.Add(Rex(register, register));
        _code.Add(MovLoadOpcode);
        _code.Add(ModRm(ModMemoryDisp8, register, register));
        _code.Add(displacement);
    }

    /// <summary><c>sub r10,r11</c>.</summary>
    public void SubR10R11()
    {
        Emit(SubR10R11Encoding);
    }

    /// <summary><c>cmp r10,imm32</c> (sign-extended).</summary>
    public void CmpR10(uint value)
    {
        Emit(CmpR10Imm32Encoding);
        Emit32(value);
    }

    /// <summary><c>jae rel8</c> with a displacement patched by <see cref="PatchJumpHere"/>; returns the patch position.</summary>
    public int JaeForward()
    {
        Emit([JaeRel8Opcode], 0);

        return _code.Count - 1;
    }

    /// <summary><c>jz rel8</c> with a displacement patched by <see cref="PatchJumpHere"/>; returns the patch position.</summary>
    public int JzForward()
    {
        Emit([JzRel8Opcode], 0);

        return _code.Count - 1;
    }

    /// <summary>Points the <c>jz</c> whose displacement byte is at <paramref name="patch"/> to the current position.</summary>
    public void PatchJumpHere(int patch)
    {
        _code[patch] = (byte)(_code.Count - (patch + 1));
    }

    /// <summary><c>mov rcx,rdx; mov rdx,r8; mov r8,r9</c>: shifts the first integer arguments one register down.</summary>
    public void ShiftIntegerArguments()
    {
        Emit(MovRcxRdxEncoding);
        Emit(MovRdxR8Encoding);
        Emit(MovR8R9Encoding);
    }

    /// <summary><c>movaps xmm0,xmm1; movaps xmm1,xmm2; movaps xmm2,xmm3</c>.</summary>
    public void ShiftFloatArguments()
    {
        Emit(MovapsXmm0Xmm1);
        Emit(MovapsXmm1Xmm2);
        Emit(MovapsXmm2Xmm3);
    }

    /// <summary><c>mov eax,[rip+disp32]</c>.</summary>
    public void LoadEaxRipRelative(int displacement)
    {
        Emit(MovEaxFromRipDisp32);
        Emit32((uint)displacement);
    }

    /// <summary><c>mov [r11],eax</c>.</summary>
    public void StoreEaxToR11()
    {
        Emit(MovR11MemFromEax);
    }

    /// <summary><c>mov rcx,[rcx+disp8]</c>.</summary>
    public void LoadRcxFromRcx(byte displacement)
    {
        Emit(MovRcxFromRcxDisp8, displacement);
    }

    /// <summary><c>mov rdx,[rdx+disp8]</c>.</summary>
    public void LoadRdxFromRdx(byte displacement)
    {
        Emit(MovRdxFromRdxDisp8, displacement);
    }

    /// <summary><c>mov rax,[rcx]</c>.</summary>
    public void LoadRaxFromRcx()
    {
        Emit(MovRaxFromRcxEncoding);
    }

    /// <summary><c>mov rax,[rdx]</c>.</summary>
    public void LoadRaxFromRdx()
    {
        Emit(MovRaxFromRdxEncoding);
    }

    /// <summary><c>xchg rcx,rdx</c>.</summary>
    public void SwapRcxRdx()
    {
        Emit(XchgRcxRdxEncoding);
    }

    /// <summary><c>jmp qword ptr [rax+disp32]</c>: a tail call through a vtable slot.</summary>
    public void JmpRaxIndirect(int displacement)
    {
        Emit(JmpRaxDisp32Encoding);
        Emit32((uint)displacement);
    }

    /// <summary>Pads with <c>int3</c> to a multiple of <paramref name="alignment"/>.</summary>
    public void PadTo(int alignment)
    {
        while (_code.Count % alignment != 0)
        {
            _code.Add(Int3Opcode);
        }
    }

    /// <summary>Appends raw data (unwind information, tables) after the code.</summary>
    public void Data(ReadOnlySpan<byte> bytes)
    {
        _code.AddRange(bytes.ToArray());
    }

    public void Data32(uint value)
    {
        Emit32(value);
    }

    /// <summary>REX.W with REX.R for an extended ModRM reg operand and REX.B for an extended r/m operand.</summary>
    private static byte Rex(int reg, int rm)
    {
        byte rex = RexW;

        if (reg >= FirstExtendedRegister)
        {
            rex |= RexR;
        }

        if (rm >= FirstExtendedRegister)
        {
            rex |= RexB;
        }

        return rex;
    }

    private static byte ModRm(byte mod, int reg, int rm)
    {
        return (byte)(mod | ((reg & LowRegisterMask) << ModRmRegShift) | (rm & LowRegisterMask));
    }

    /// <summary>A base of rsp/r12 needs a SIB byte and rbp/r13 a displacement; neither is emitted here.</summary>
    private static void CheckPlainBase(int register)
    {
        int low = register & LowRegisterMask;

        if (low == Rsp || low == Rbp)
        {
            throw new ArgumentOutOfRangeException(nameof(register), register, "rsp, rbp, r12 and r13 need another encoding");
        }
    }

    private void Emit(byte[] encoding)
    {
        _code.AddRange(encoding);
    }

    private void Emit(byte[] encoding, byte operand)
    {
        _code.AddRange(encoding);
        _code.Add(operand);
    }

    private void Emit32(uint value)
    {
        Span<byte> bytes = stackalloc byte[sizeof(uint)];
        BinaryPrimitives.WriteUInt32LittleEndian(bytes, value);
        _code.AddRange(bytes.ToArray());
    }

    private void Emit64(ulong value)
    {
        Span<byte> bytes = stackalloc byte[sizeof(ulong)];
        BinaryPrimitives.WriteUInt64LittleEndian(bytes, value);
        _code.AddRange(bytes.ToArray());
    }
}
