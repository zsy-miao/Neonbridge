// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Formats.Tests.Support;

/// <summary>
/// A hand-built PE32+ image for the exe-analysis readers: one <c>.text</c> section with a <c>call</c> and a
/// <c>jmp</c> through one import slot, the <c>RPF9</c> magic and a <c>shaders.rpf</c> string, an import of
/// <c>TEST.dll!TestFunc</c>, a TLS callback and an exception directory. All structures live in the section, at the
/// RVAs the readers resolve through the section table.
/// </summary>
public static class SyntheticPe
{
    public const ulong ImageBase = 0x140000000;
    public const uint EntryPoint = 0x1000;
    public const uint IatRva = 0x1100;
    public const uint Rpf9Rva = 0x100C;
    public const uint ShaderRva = 0x1010;
    public const uint TlsCallbackRva = 0x1000;

    private const int NewHeaderPointer = 0x3C;
    private const int PeHeader = 0x80;
    private const int Coff = 0x84;
    private const int Optional = 0x98;
    private const int OptionalSize = 0xF0;
    private const int SectionTable = Optional + OptionalSize;
    private const int RawBase = 0x200;
    private const int Size = 0x600;

    public static byte[] Build()
    {
        var data = new byte[Size];
        WriteHeaders(data);
        WriteSection(data);
        WriteCode(data);
        WriteImports(data);
        WriteTls(data);

        return data;
    }

    private static int Offset(uint rva)
    {
        return RawBase + (int)(rva - EntryPoint);
    }

    private static void WriteHeaders(byte[] data)
    {
        U32(data, NewHeaderPointer, PeHeader);
        Encoding.ASCII.GetBytes("PE\0\0").CopyTo(data, PeHeader);
        U16(data, Coff, 0x8664);
        U16(data, Coff + 2, 1);
        U32(data, Coff + 4, 0x1234);
        U16(data, Coff + 16, OptionalSize);

        U16(data, Coff + 18, 0x2022);
        WriteOptional(data);
    }

    private static void WriteOptional(byte[] data)
    {
        U16(data, Optional, 0x20B);
        U32(data, Optional + 16, EntryPoint);
        U64(data, Optional + 24, ImageBase);
        U32(data, Optional + 36, RawBase);
        U16(data, Optional + 68, 2);
        U16(data, Optional + 70, 0x4160);

        U32(data, Optional + 108, 16);
        Directory(data, 1, 0x1200, 0x28);
        Directory(data, 3, 0x1380, 0x24);
        Directory(data, 9, 0x1300, 0x28);
    }

    private static void Directory(byte[] data, int index, uint rva, uint size)
    {
        var at = Optional + 112 + (index * 8);
        U32(data, at, rva);
        U32(data, at + 4, size);
    }

    private static void WriteSection(byte[] data)
    {
        Encoding.ASCII.GetBytes(".text").CopyTo(data, SectionTable);
        U32(data, SectionTable + 8, 0x400);
        U32(data, SectionTable + 12, EntryPoint);
        U32(data, SectionTable + 16, 0x400);
        U32(data, SectionTable + 20, RawBase);
    }

    private static void WriteCode(byte[] data)
    {
        var call = Offset(EntryPoint);
        data[call] = 0xFF;
        data[call + 1] = 0x15;
        U32(data, call + 2, 0xFA);

        data[call + 6] = 0xFF;
        data[call + 7] = 0x25;
        U32(data, call + 8, 0xF4);
        Encoding.ASCII.GetBytes("RPF9").CopyTo(data, Offset(Rpf9Rva));
        Encoding.ASCII.GetBytes("shaders.rpf").CopyTo(data, Offset(ShaderRva));
    }

    private static void WriteImports(byte[] data)
    {
        U64(data, Offset(IatRva), 0x1240);
        U32(data, Offset(0x1200) + 12, 0x1260);
        U32(data, Offset(0x1200) + 16, IatRva);
        Encoding.ASCII.GetBytes("TestFunc").CopyTo(data, Offset(0x1242));
        Encoding.ASCII.GetBytes("TEST.dll").CopyTo(data, Offset(0x1260));
    }

    private static void WriteTls(byte[] data)
    {
        U64(data, Offset(0x1300) + 24, ImageBase + 0x1340);
        U64(data, Offset(0x1340), ImageBase + TlsCallbackRva);
    }

    private static void U16(byte[] data, int at, ushort value)
    {
        BinaryPrimitives.WriteUInt16LittleEndian(data.AsSpan(at), value);
    }

    private static void U32(byte[] data, int at, uint value)
    {
        BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(at), value);
    }

    private static void U64(byte[] data, int at, ulong value)
    {
        BinaryPrimitives.WriteUInt64LittleEndian(data.AsSpan(at), value);
    }
}
