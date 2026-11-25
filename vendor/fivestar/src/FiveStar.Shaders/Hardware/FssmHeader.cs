// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Hardware;

/// <summary>
/// Decodes the header of an FSSM program (the native Xbox Series / RDNA program). Offsets are file offsets; offset
/// values stored in the header are relative to FSSM+0x10. docs/fssm.md lists every field.
/// </summary>
public static class FssmHeader
{
    /// <summary>Program magic at offset 0.</summary>
    public const string Magic = "FSSM";

    /// <summary>Stored offsets are relative to this file offset.</summary>
    public const int OffsetBase = 0x10;

    // Generic program block.
    private const int CodeOffsetField = 0xC0;
    private const int CodeSizeField = 0xC8;
    private const int FlagsField = 0xD4;
    private const int ResourceRecordCountField = 0xF8;
    private const int StageField = 0x100;
    private const int FloatModeField = 0x104;
    private const int LdsBytesField = 0x10C;
    private const int RegisterCountsField = 0x110;

    // Flags @0xD4: bit 5 wave32, bits 16..23 USER_SGPR count.
    private const uint Wave32Flag = 0x20;
    private const int UserSgprShift = 16;
    private const uint ByteMask = 0xFF;
    private const int Wave32 = 32;
    private const int Wave64 = 64;

    // Register counts @0x110: low 16 bits VGPRs, bits 20..31 SGPRs (incl. VCC).
    private const uint LowHalfMask = 0xFFFF;
    private const int SgprCountShift = 20;

    // Geometry programs carry the VS copy shader as a second program block.
    private const int CopyShaderBlockField = 0x148;

    /// <summary>True when the bytes start with the FSSM magic.</summary>
    public static bool HasMagic(ReadOnlySpan<byte> bytes)
    {
        return bytes.Length >= Magic.Length && Encoding.Latin1.GetString(bytes[..Magic.Length]) == Magic;
    }

    /// <summary>Decodes the generic block and the block of the program's stage.</summary>
    public static ShaderConfig Decode(ReadOnlySpan<byte> program)
    {
        if (!HasMagic(program))
        {
            throw new InvalidDataException("not an FSSM blob");
        }

        var stage = (HardwareStage)program[StageField];
        var flags = U32(program, FlagsField);
        var counts = U32(program, RegisterCountsField);
        var wave = (flags & Wave32Flag) != 0 ? Wave32 : Wave64;

        // Legacy VS, NGG and GS always run as wave64.
        if (stage is HardwareStage.Vertex or HardwareStage.Ngg or HardwareStage.Geometry)
        {
            wave = Wave64;
        }

        var config = new ShaderConfig
        {
            Stage = stage,
            CodeOffset = (long)U64(program, CodeOffsetField) + OffsetBase,
            CodeSize = (long)U64(program, CodeSizeField),
            Flags = flags,
            WaveSize = wave,
            UserSgprs = (int)((flags >> UserSgprShift) & ByteMask),
            FloatMode = (int)(U32(program, FloatModeField) & ByteMask),
            LdsBytes = (int)U32(program, LdsBytesField),
            VgprCount = (int)(counts & LowHalfMask),
            SgprCount = (int)(counts >> SgprCountShift),
            ResourceRecordCount = (int)U32(program, ResourceRecordCountField),
        };

        return WithStageBlock(config, program, stage, flags);
    }

    /// <summary>The main program's code: the code block without its 24-byte hash prefix and trailing s_code_end padding.</summary>
    public static byte[] ProgramCode(ReadOnlySpan<byte> program)
    {
        // The code block starts with 24 bytes carrying the shader hash; s_code_end (0xBF9F0000) pads its end.
        const int HashPrefix = 0x18;
        const uint CodeEnd = 0xBF9F0000;
        const int WordSize = 4;

        var start = (int)U64(program, CodeOffsetField) + OffsetBase;
        var end = Math.Min(start + (int)U64(program, CodeSizeField), program.Length);
        var from = Math.Min(start + HashPrefix, end);

        while (end - from >= WordSize && U32(program, end - WordSize) == CodeEnd)
        {
            end -= WordSize;
        }

        return program[from..end].ToArray();
    }

    /// <summary>The whole second program block of a geometry program (the driver's VS copy shader).</summary>
    public static byte[] CopyShaderCode(ReadOnlySpan<byte> program)
    {
        const int SizeField = CopyShaderBlockField + sizeof(ulong);

        var start = Math.Min((int)U64(program, CopyShaderBlockField) + OffsetBase, program.Length);
        var end = Math.Min(start + (int)U64(program, SizeField), program.Length);

        return program[start..end].ToArray();
    }

    private static ShaderConfig WithStageBlock(ShaderConfig config, ReadOnlySpan<byte> program, HardwareStage stage, uint flags)
    {
        switch (stage)
        {
            case HardwareStage.Pixel:
                return config with { Pixel = PixelBlock.Decode(program, config.UserSgprs ?? 0) };
            case HardwareStage.Vertex:
                return config with { Vertex = VertexBlock.Decode(program, flags) };
            case HardwareStage.Compute:
                return ComputeBlock.Apply(config, program);
            case HardwareStage.Ngg:
                return config with { Ngg = NggBlock.Decode(program) };
            case HardwareStage.Geometry:
                return config with { CopyShaderBlock = CopyShaderBlockField };
            default:
                return config;
        }
    }

    internal static uint U32(ReadOnlySpan<byte> program, int at)
    {
        return BinaryPrimitives.ReadUInt32LittleEndian(program[at..]);
    }

    private static ulong U64(ReadOnlySpan<byte> program, int at)
    {
        return BinaryPrimitives.ReadUInt64LittleEndian(program[at..]);
    }
}
