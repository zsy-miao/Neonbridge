// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Cache;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// Cache input layouts map to the attributes a translated vertex shader reads by semantic, found in the ISG1 signature
/// of the program's DXBC container, not by their position in the cache record.
/// </summary>
public sealed class D3D12XInputLayoutTests
{
    private const string Program = "sgd0_00000000_t32_VS_Test_Wrapped";
    private const string BinkVertex = "_VS_Bink_e9aa19fd_Wrapped";

    [Fact]
    public void A_cache_layout_listed_in_another_order_than_the_signature_maps_by_semantic()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-layout-");

        try
        {
            // The shader reads SV_VertexID, then POSITION, TEXCOORD and COLOR; the cache lists COLOR before TEXCOORD
            // and a NORMAL the shader does not read.
            var programs = Directory.CreateDirectory(Path.Combine(work.FullName, "fssm")).FullName;
            Directory.CreateDirectory(Path.Combine(work.FullName, "dxbc"));
            File.WriteAllBytes(Path.Combine(work.FullName, "dxbc", Program + ".dxbc"), Container(("SV_VertexID", 6), ("POSITION", 0), ("TEXCOORD", 0), ("COLOR", 0)));
            VertexElement[] layout = [Element("POSITION", 0), Element("COLOR", 12), Element("TEXCOORD", 16), Element("NORMAL", 24)];
            var signatures = new ProgramSignatures(programs);

            var mapped = InputLayoutMapping.Map(layout, signatures.Inputs(Program));
            var positional = InputLayoutMapping.Map(layout, signatures.Inputs("missing"));

            Assert.Equal(["ATTR0", "ATTR2", "ATTR1", "UNUSED0"], mapped.Select(element => element.Semantic + element.SemanticIndex));
            Assert.Equal([0, 12, 16, 24], mapped.Select(element => element.Offset));
            Assert.Equal(["ATTR0", "ATTR1", "ATTR2", "ATTR3"], positional.Select(element => element.Semantic + element.SemanticIndex));
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    [DumpFact]
    public void The_bink_video_pipeline_feeds_texcoord_and_colour_to_the_attributes_its_shader_reads()
    {
        if (PipelineFactAttribute.ShaderFolder is null)
        {
            return;
        }

        var work = Directory.CreateTempSubdirectory("fivestar-layout-");

        try
        {
            var programs = Directory.CreateDirectory(Path.Combine(work.FullName, "fssm")).FullName;
            var containers = Directory.CreateDirectory(Path.Combine(work.FullName, "dxbc")).FullName;
            var name = CopyProgram(BinkVertex, programs, containers);
            var cache = DumpPipelineCache.Read();
            var index = ProgramIndex.Load(programs);

            var record = cache.Graphics.Single(pipeline => Runs(cache, index, pipeline, name));
            var layout = InputLayoutParser.Parse(cache.Meta(record.InputLayoutMeta).Span)
                .Select(element => new VertexElement(element.Semantic, element.SemanticIndex, element.Format, element.InputSlot, element.Offset, element.Classification, element.StepRate))
                .ToList();

            var mapped = InputLayoutMapping.Map(layout, new ProgramSignatures(programs).Inputs(name));

            Assert.Equal(["POSITION", "COLOR", "TEXCOORD"], layout.Select(element => element.Semantic));
            Assert.Equal(["ATTR0", "ATTR2", "ATTR1"], mapped.Select(element => element.Semantic + element.SemanticIndex));
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    private static VertexElement Element(string semantic, int offset)
    {
        return new VertexElement(semantic, 0, 2, 0, offset, 0, 0);
    }

    private static bool Runs(PipelineCache cache, ProgramIndex index, GraphicsPipelineRecord pipeline, string vertex)
    {
        var link = pipeline.Stage(PipelineSlot.Vertex);

        if (!link.HasValue)
        {
            return false;
        }

        return index.Find(cache.Shader(link.Value.Shader)!.Value.Span).Contains(vertex);
    }

    private static string CopyProgram(string part, string programs, string containers)
    {
        var source = PipelineFactAttribute.ShaderFolder!;
        var path = Directory.EnumerateFiles(source, $"*{part}.fssm").Single();
        var name = Path.GetFileNameWithoutExtension(path);
        File.Copy(path, Path.Combine(programs, name + ".fssm"));
        File.Copy(Path.Combine(Path.GetDirectoryName(Path.GetFullPath(source))!, "dxbc", name + ".dxbc"), Path.Combine(containers, name + ".dxbc"));

        return name;
    }

    /// <summary>A DXBC container holding one ISG1 chunk whose elements sit in registers 0, 1, ... in order.</summary>
    private static byte[] Container(params (string Name, int SystemValue)[] elements)
    {
        var table = 8 + (32 * elements.Length);
        var names = elements.Select(element => Encoding.ASCII.GetBytes(element.Name + "\0")).ToList();
        var signature = new byte[table + names.Sum(name => name.Length)];
        BinaryPrimitives.WriteInt32LittleEndian(signature, elements.Length);
        BinaryPrimitives.WriteInt32LittleEndian(signature.AsSpan(4), 8);

        var cursor = table;

        for (var index = 0; index < elements.Length; index++)
        {
            var row = signature.AsSpan(8 + (32 * index));
            BinaryPrimitives.WriteInt32LittleEndian(row[4..], cursor);
            BinaryPrimitives.WriteInt32LittleEndian(row[12..], elements[index].SystemValue);
            BinaryPrimitives.WriteInt32LittleEndian(row[16..], 3);
            BinaryPrimitives.WriteInt32LittleEndian(row[20..], index);

            row[24] = 0xF;
            names[index].CopyTo(signature, cursor);
            cursor += names[index].Length;
        }

        var container = new byte[0x24 + 8 + signature.Length];
        Encoding.ASCII.GetBytes("DXBC").CopyTo(container, 0);
        BinaryPrimitives.WriteInt32LittleEndian(container.AsSpan(0x1C), 1);
        BinaryPrimitives.WriteInt32LittleEndian(container.AsSpan(0x20), 0x24);

        Encoding.ASCII.GetBytes("ISG1").CopyTo(container, 0x24);
        BinaryPrimitives.WriteInt32LittleEndian(container.AsSpan(0x28), signature.Length);
        signature.CopyTo(container, 0x2C);

        return container;
    }
}
