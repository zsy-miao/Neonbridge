// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using Xunit;
using Xunit.Abstractions;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The pipeline slots on WARP with real data: the immediate-mode unlit pass of the dump's pipeline cache, its two
/// programs translated ahead of time, created through the cache slot (57), the stream slot (86) with the programs'
/// DXBC containers, and the derive slot (55) with a rasterizer override.
/// </summary>
public sealed unsafe class D3D12XPipelineWarpTests(ITestOutputHelper output)
{
    private const string VertexPart = "_VS_TransformUnlit_IM_";
    private const string PixelPart = "_PS_TexturedUnlit_IM_";
    private const int Rgba8Unorm = 28;
    private const int D32Float = 40;

    [PipelineFact]
    public void A_cache_record_a_stream_and_a_derived_pipeline_create_on_warp()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-pso-");

        try
        {
            var cache = DumpPipelineCache.Read();
            var programs = Path.Combine(work.FullName, "fssm");
            var index = Candidates(programs, Path.Combine(work.FullName, "dxbc"));
            var (record, vertex, pixel) = UnlitRecord(cache, index);

            var translated = Path.Combine(work.FullName, "out");
            Translate([vertex, pixel], programs, translated);
            output.WriteLine($"record {record.Index}: {vertex} + {pixel}");

            Create(cache, record, new TranslatedProgramLibrary(index, translated, null), new ProgramSignatures(programs), Path.Combine(work.FullName, "dxbc", vertex + ".dxbc"), Path.Combine(work.FullName, "dxbc", pixel + ".dxbc"));
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    private static void Create(PipelineCache cache, GraphicsPipelineRecord record, IProgramLibrary library, ProgramSignatures signatures, string vertexContainer, string pixelContainer)
    {
        using var device = GpuDevice.CreateWarp(debugLayer: false);
        using var memory = new NativeBlocks();
        var log = new StringWriter();

        var warnings = new OnceLog(new LoaderLog(log, null, false));
        var (device2, root) = DesktopObjects(device.Device.NativePointer);
        var builds = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
        var slots = new PipelineSlots(new PipelineFactory(device2, root, library, signatures, warnings, null, new StandInPipelines(PipelineSlots.CreationBudget, builds, builds, warnings), null), warnings);
        var iid = DesktopDevice.PipelineStateIid;
        nint cached, streamed, derived;

        var cachedResult = slots.CachedGraphics(CachedDesc(memory, cache, record), &iid, &cached);
        var layout = InputLayoutParser.Parse(cache.Meta(record.InputLayoutMeta).Span);
        var streamResult = slots.Stream(StreamDesc(memory, File.ReadAllBytes(vertexContainer), File.ReadAllBytes(pixelContainer), layout), &iid, &streamed);
        var derivedResult = slots.Derive(cached, 1, RasterizerOverride(memory), &iid, &derived);

        Assert.True(cachedResult == 0 && cached != 0, $"slot 57: 0x{cachedResult:x8} {log}");
        Assert.True(streamResult == 0 && streamed != 0, $"slot 86: 0x{streamResult:x8} {log}");
        Assert.True(derivedResult == 0 && derived != 0 && derived != cached, $"slot 55: 0x{derivedResult:x8} {log}");

        foreach (var unknown in new[] { cached, streamed, derived, root, device2 })
        {
            DesktopDevice.Release(unknown);
        }
    }

    private static (nint Device2, nint Root) DesktopObjects(nint device)
    {
        var iid = DesktopDevice.Device2Iid;
        nint device2, root;

        Assert.Equal(0, DesktopDevice.QueryInterface(device, &iid, &device2));
        Assert.Equal(0, BindlessRoot.Create(device, &root));

        return (device2, root);
    }

    /// <summary>The 0x108-byte description the game's cache loader (0x14248f680) builds for slot 57.</summary>
    private static byte* CachedDesc(NativeBlocks memory, PipelineCache cache, GraphicsPipelineRecord record)
    {
        var desc = memory.Alloc(0x108);
        memory.Range(desc + 0x08, cache.Meta(record.StateMeta).Span);
        memory.Range(desc + 0x18, cache.Meta(record.InputLayoutMeta).Span);

        foreach (var link in record.Stages)
        {
            var slot = desc + 0x28 + ((int)link.Slot * 0x20);
            memory.Range(slot, cache.Shader(link.Shader)!.Value.Span);
            memory.Range(slot + 0x10, cache.Meta(link.Meta).Span);
        }

        return desc;
    }

    /// <summary>A D3D12_PIPELINE_STATE_STREAM_DESC over a stream of VS, PS, input layout, topology and formats.</summary>
    private static byte* StreamDesc(NativeBlocks memory, byte[] vertex, byte[] pixel, IReadOnlyList<InputElement> layout)
    {
        var stream = memory.Alloc(0xA0);
        (int Offset, int Type, int Value)[] words = [(0x10, 1, 0), (0x28, 2, 0), (0x40, 12, 0), (0x58, 14, 3), (0x60, 15, Rgba8Unorm), (0x88, 16, D32Float), (0x90, 17, 1)];

        foreach (var (offset, type, value) in words)
        {
            *(int*)(stream + offset) = type;
            *(int*)(stream + offset + 4) = value;
        }

        memory.Range(stream + 0x18, vertex);
        memory.Range(stream + 0x30, pixel);
        *(nint*)(stream + 0x48) = (nint)Elements(memory, layout);
        *(int*)(stream + 0x50) = layout.Count;
        *(int*)(stream + 0x84) = 1;

        var desc = memory.Alloc(16);
        *(long*)desc = 0xA0;
        *(nint*)(desc + 8) = (nint)stream;

        return desc;
    }

    private static byte* Elements(NativeBlocks memory, IReadOnlyList<InputElement> layout)
    {
        var elements = memory.Alloc(layout.Count * 0x20);

        for (var index = 0; index < layout.Count; index++)
        {
            var element = layout[index];
            var row = (int*)(elements + (index * 0x20));
            *(nint*)row = (nint)memory.Ansi(element.Semantic);
            (row[2], row[3], row[4], row[5], row[6], row[7]) = (element.SemanticIndex, element.Format, element.InputSlot, element.Offset, element.Classification, element.StepRate);
        }

        return elements;
    }

    /// <summary>One override entry: kind 0, a rasterizer desc with no culling.</summary>
    private static byte* RasterizerOverride(NativeBlocks memory)
    {
        var entry = memory.Alloc(0x14C);
        var rasterizer = (int*)(entry + 4);
        (rasterizer[0], rasterizer[1], rasterizer[6]) = (3, 1, 1);

        return entry;
    }

    /// <summary>Copies the unlit programs (and their containers) of the shader folder and indexes them.</summary>
    private static ProgramIndex Candidates(string programs, string containers)
    {
        var source = PipelineFactAttribute.ShaderFolder!;
        var sourceContainers = Path.Combine(Path.GetDirectoryName(Path.GetFullPath(source))!, "dxbc");
        Directory.CreateDirectory(programs);
        Directory.CreateDirectory(containers);

        foreach (var path in new[] { VertexPart, PixelPart }.SelectMany(part => Directory.EnumerateFiles(source, $"*{part}*.fssm")))
        {
            var name = Path.GetFileNameWithoutExtension(path);
            File.Copy(path, Path.Combine(programs, name + ".fssm"));
            File.Copy(Path.Combine(sourceContainers, name + ".dxbc"), Path.Combine(containers, name + ".dxbc"));
        }

        return ProgramIndex.Load(programs);
    }

    private static (GraphicsPipelineRecord Record, string Vertex, string Pixel) UnlitRecord(PipelineCache cache, ProgramIndex index)
    {
        foreach (var record in cache.Graphics)
        {
            var vertex = Program(cache, index, record, PipelineSlot.Vertex);
            var pixel = Program(cache, index, record, PipelineSlot.Pixel);

            if (vertex is not null && pixel is not null && record.Stages.Count == 2)
            {
                return (record, vertex, pixel);
            }
        }

        throw new InvalidDataException("no unlit pipeline in the cache");
    }

    private static string? Program(PipelineCache cache, ProgramIndex index, GraphicsPipelineRecord record, PipelineSlot slot)
    {
        var link = record.Stage(slot);

        if (!link.HasValue)
        {
            return null;
        }

        var names = index.Find(cache.Shader(link.Value.Shader)!.Value.Span);

        if (names.Count == 0)
        {
            return null;
        }

        return names[0];
    }

    private static void Translate(IReadOnlyList<string> names, string programs, string output)
    {
        var compiler = new DxcCompiler(DxcOptions.Defaults(PipelineFactAttribute.Dxc, retry: false));
        var paths = names.Select(name => Path.Combine(programs, name + ".fssm")).ToList();
        var results = new CorpusTranslator(new ProgramLoader(new RdnaDecoder()), compiler, null, null, null)
            .Run(paths, new CorpusOptions(output, null, names.Count, null));

        Assert.All(results, result => Assert.Equal(ProgramStatus.Ok, result.Status));
    }
}
