// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.IO.Compression;
using System.Text;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Hardware;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>The shader containers and the FSSM header decode from hand-built bytes.</summary>
public sealed class ContainerTests
{
    [Fact]
    public void A_game_container_yields_its_signature_runtime_info_and_native_program()
    {
        var program = Fssm(stage: 6, flags: 0x20);
        var container = Dxbc(("ISG1", Signature()), ("PSV0", Psv0()), ("DXIL", DxilWithXdat(program)));

        var parsed = DxbcContainer.Parse(container);
        var inputs = SignatureParser.Parse(parsed.Find("ISG1")!.Data.Span);
        var psv = PipelineStateParser.Parse(parsed.Find("PSV0")!.Data.Span);
        var extracted = ShaderArchiveExtractor.ReadProgram(container);

        Assert.Equal(["ISG1", "PSV0", "DXIL"], parsed.Chunks.Select(chunk => chunk.FourCc));
        Assert.Equal(new SignatureElement("TEXCOORD", 3, string.Empty, "float", 1, "xy", 3, 0), Assert.Single(inputs));
        Assert.Equal("cs", psv.Stage);
        Assert.Equal(new ThreadGroupSize(8, 8, 1), psv.NumThreads);
        Assert.Equal(program, extracted);
    }

    [Fact]
    public void A_truncated_or_inconsistent_dxbc_header_is_refused()
    {
        var magic = Encoding.ASCII.GetBytes(DxbcContainer.Magic);

        Assert.Throws<InvalidDataException>(() => DxbcContainer.Parse(magic));

        var count = new byte[DxbcContainer.HeaderSize];

        magic.CopyTo(count, 0);
        BinaryPrimitives.WriteInt32LittleEndian(count.AsSpan(DxbcContainer.TotalSizeOffset), count.Length);
        BinaryPrimitives.WriteInt32LittleEndian(count.AsSpan(0x1C), 2);
        Assert.Throws<InvalidDataException>(() => DxbcContainer.Parse(count));
    }

    [Fact]
    public void A_dxbc_chunk_inside_the_header_or_with_a_negative_size_is_refused()
    {
        var inside = new byte[DxbcContainer.HeaderSize + 16];

        Encoding.ASCII.GetBytes(DxbcContainer.Magic).CopyTo(inside, 0);
        BinaryPrimitives.WriteInt32LittleEndian(inside.AsSpan(DxbcContainer.TotalSizeOffset), inside.Length);
        BinaryPrimitives.WriteInt32LittleEndian(inside.AsSpan(0x1C), 1);
        BinaryPrimitives.WriteInt32LittleEndian(inside.AsSpan(0x20), 0x10);
        Assert.Throws<InvalidDataException>(() => DxbcContainer.Parse(inside));

        var negative = new byte[DxbcContainer.HeaderSize + 16];

        Encoding.ASCII.GetBytes(DxbcContainer.Magic).CopyTo(negative, 0);
        BinaryPrimitives.WriteInt32LittleEndian(negative.AsSpan(DxbcContainer.TotalSizeOffset), negative.Length);
        BinaryPrimitives.WriteInt32LittleEndian(negative.AsSpan(0x1C), 1);
        BinaryPrimitives.WriteInt32LittleEndian(negative.AsSpan(0x20), DxbcContainer.HeaderSize);
        BinaryPrimitives.WriteInt32LittleEndian(negative.AsSpan(DxbcContainer.HeaderSize + 4), -1);
        Assert.Throws<InvalidDataException>(() => DxbcContainer.Parse(negative));
    }

    [Fact]
    public void A_mesh_runtime_info_reads_the_topology_after_the_primitive_vector_count()
    {
        var bytes = new byte[4 + 48];
        BinaryPrimitives.WriteInt32LittleEndian(bytes, 48);
        bytes[4 + 24] = (byte)DxilShaderKind.Mesh;

        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(4 + 12), 96);
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(4 + 14), 64);
        bytes[4 + 26] = 1; // SigPrimVectors
        bytes[4 + 27] = 2; // MeshOutputTopology: triangle

        var psv = PipelineStateParser.Parse(bytes);

        Assert.Equal("ms", psv.Stage);
        Assert.Equal(96, psv.MaxVertices);
        Assert.Equal(64, psv.MaxPrimitives);
        Assert.Equal("triangle", psv.Topology);
    }

    [Fact]
    public void A_compute_header_decodes_wave_size_system_registers_and_code()
    {
        var program = Fssm(stage: 6, flags: 0x20);

        var config = FssmHeader.Decode(program);
        var code = FssmHeader.ProgramCode(program);

        Assert.Equal(HardwareStage.Compute, config.Stage);
        Assert.Equal(32, config.WaveSize);
        Assert.Equal(2, config.UserSgprs);

        Assert.Equal(new ThreadGroupSize(64, 1, 1), config.Compute!.NumThreads);
        Assert.Equal([new GroupIdSgpr('x', 2), new GroupIdSgpr('y', 3)], config.Compute.GroupIdSgprs);
        Assert.Equal(1, config.Compute.ThreadIdComponentCount);
        Assert.Equal([0x00, 0x00, 0x81, 0xBF], code);
    }

    [Fact]
    public void Records_behind_a_false_candidate_and_stored_records_are_found()
    {
        var container = Dxbc(("DXIL", DxilWithXdat(Fssm(stage: 6, flags: 0x20))));

        // "K,Q", NUL and the next record's length byte '3' look like a record header with tag 0x33 (shaders.rpf
        // 0x6e18ed5); the game executable stores its clear shaders uncompressed.
        byte[] file = [0xE0, .. "K,Q"u8, 0, .. Compressed("PS_EarlyComposite_HeatHaze_Simple_467d9c8c_Wrapped", container), .. Stored("VS_Clear_f05fef9c_Wrapped", container)];

        var records = ShaderArchiveScanner.ScanFile(file, ShaderArchiveScanner.ExecutableRegion);

        Assert.Equal(["PS_EarlyComposite_HeatHaze_Simple_467d9c8c_Wrapped", "VS_Clear_f05fef9c_Wrapped"], records.Select(record => record.Name));
        Assert.All(records, record => Assert.Equal(container, record.Container));
        Assert.Equal(0u, records[1].Constant);
    }

    // u8 name length + 1, name, NUL, tag 0x32, then the record's sizes and data.
    private static byte[] Compressed(string name, byte[] container)
    {
        using var compressed = new MemoryStream();

        using (var deflate = new DeflateStream(compressed, CompressionLevel.Optimal, leaveOpen: true))
        {
            deflate.Write(container);
        }

        var data = compressed.ToArray();
        var sizes = new byte[16];
        BinaryPrimitives.WriteInt32LittleEndian(sizes, data.Length);
        BinaryPrimitives.WriteUInt32LittleEndian(sizes.AsSpan(4), 0x000DDCE7);
        BinaryPrimitives.WriteInt32LittleEndian(sizes.AsSpan(8), data.Length);
        BinaryPrimitives.WriteInt32LittleEndian(sizes.AsSpan(12), container.Length);

        return [.. Header(name), .. sizes, .. data];
    }

    private static byte[] Stored(string name, byte[] container)
    {
        var size = new byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(size, container.Length);

        return [.. Header(name), .. size, .. container];
    }

    private static byte[] Header(string name)
    {
        return [(byte)(name.Length + 1), .. Encoding.ASCII.GetBytes(name), 0, 0x32];
    }

    // FSSM: code block at 0x400 (stored offset 0x3F0): 24-byte hash prefix, s_endpgm, one s_code_end pad.
    private static byte[] Fssm(byte stage, uint flags)
    {
        var bytes = new byte[0x600];
        Encoding.ASCII.GetBytes("FSSM").CopyTo(bytes, 0);
        BinaryPrimitives.WriteUInt64LittleEndian(bytes.AsSpan(0xC0), 0x3F0);
        BinaryPrimitives.WriteUInt64LittleEndian(bytes.AsSpan(0xC8), 0x18 + 8);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(0xD4), flags);
        bytes[0x100] = stage;

        // COMPUTE_PGM_RSRC2: USER_SGPR 2, TGID_X and TGID_Y, TIDIG_COMP_CNT 1; numthreads 64x1x1.
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(0x138), (2u << 1) | (1u << 7) | (1u << 8) | (1u << 11));
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(0x13C), 64);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(0x140), 1);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(0x144), 1);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(0x400 + 0x18), 0xBF810000);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(0x400 + 0x1C), 0xBF9F0000);

        return bytes;
    }

    // One element: TEXCOORD3, register 1, mask xy, float.
    private static byte[] Signature()
    {
        var bytes = new byte[8 + 32 + 16];
        BinaryPrimitives.WriteInt32LittleEndian(bytes, 1);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(4), 8);

        // Element: name offset 40, semantic index 3, float, register 1, mask and read/write mask xy.
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(8 + 4), 40);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(8 + 8), 3);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(8 + 16), 3);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(8 + 20), 1);
        bytes[8 + 24] = 0x3;
        bytes[8 + 25] = 0x3;

        Encoding.ASCII.GetBytes("TEXCOORD").CopyTo(bytes, 40);

        return bytes;
    }

    // PSVRuntimeInfo2 (48 bytes): stage 5 (cs) at +24, numthreads at +36.
    private static byte[] Psv0()
    {
        var bytes = new byte[4 + 48];
        BinaryPrimitives.WriteInt32LittleEndian(bytes, 48);
        bytes[4 + 24] = 5;
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(4 + 36), 8);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(4 + 40), 8);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(4 + 44), 1);

        return bytes;
    }

    // DXIL program header (24 bytes), then XDAT with one section 0x0D holding the zlib-compressed program.
    private static byte[] DxilWithXdat(byte[] program)
    {
        using var compressed = new MemoryStream();

        using (var zlib = new ZLibStream(compressed, CompressionLevel.Optimal, leaveOpen: true))
        {
            zlib.Write(program);
        }

        var section = compressed.ToArray();
        var xdat = new byte[0x18 + section.Length];
        Encoding.ASCII.GetBytes("XDAT").CopyTo(xdat, 0);
        BinaryPrimitives.WriteInt32LittleEndian(xdat.AsSpan(4), 1);
        BinaryPrimitives.WriteInt32LittleEndian(xdat.AsSpan(8), 0x18);

        BinaryPrimitives.WriteInt32LittleEndian(xdat.AsSpan(0x0C), 0x0D);
        BinaryPrimitives.WriteInt32LittleEndian(xdat.AsSpan(0x10), 0x18);
        BinaryPrimitives.WriteInt32LittleEndian(xdat.AsSpan(0x14), section.Length);
        section.CopyTo(xdat, 0x18);

        var dxil = new byte[0x18 + xdat.Length];
        Encoding.ASCII.GetBytes("DXIL").CopyTo(dxil, 8);
        xdat.CopyTo(dxil, 0x18);

        return dxil;
    }

    private static byte[] Dxbc(params (string FourCc, byte[] Data)[] chunks)
    {
        var head = 0x20 + (4 * chunks.Length);
        var bytes = new byte[head + chunks.Sum(chunk => 8 + chunk.Data.Length)];
        Encoding.ASCII.GetBytes("DXBC").CopyTo(bytes, 0);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(0x18), bytes.Length);
        BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(0x1C), chunks.Length);
        var cursor = head;

        for (var index = 0; index < chunks.Length; index++)
        {
            BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(0x20 + (4 * index)), cursor);
            Encoding.ASCII.GetBytes(chunks[index].FourCc).CopyTo(bytes, cursor);
            BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(cursor + 4), chunks[index].Data.Length);
            chunks[index].Data.CopyTo(bytes, cursor + 8);
            cursor += 8 + chunks[index].Data.Length;
        }

        return bytes;
    }
}
