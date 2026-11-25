// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Pipelines;
using FiveStar.Graphics.Rendering;
using Vortice.Direct3D;
using Vortice.Direct3D12;
using Vortice.Mathematics;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// Draws <see cref="UnlitScene"/>-layout triangle lists with the translated unlit pass: the cache's blend and rasterizer
/// state and input layout, a depth test (<see cref="CacheStateMapping.DepthTested"/>), the bindless root signature
/// and one descriptor table of <see cref="UnlitBindings"/> per texture.
/// </summary>
public static class UnlitRenderer
{
    /// <summary>The colour the target is cleared to (opaque dark grey).</summary>
    public static readonly Color4 Clear = new(0.125f, 0.125f, 0.125f, 1);

    private const float AlphaScale = 1;
    private const float ColourScale = 1;

    public static SampleFrame Render(GpuDevice gpu, SamplePipeline pipeline, TranslatedShader vertex, TranslatedShader pixel, TextureData texture)
    {
        var scene = new UnlitScene(texture.Width, texture.Height);
        var image = Draw(gpu, pipeline, vertex, pixel, [new DrawBatch(texture, scene.Vertices())], scene.Width, scene.Height);

        return new SampleFrame(scene, image.Pixels, image.Errors);
    }

    /// <summary>The depth convention <see cref="Draw"/> uses with this pipeline; vertices carry depth in it.</summary>
    public static DepthConvention Depth(SamplePipeline pipeline)
    {
        return CacheStateMapping.DepthTested(pipeline.State).Depth;
    }

    /// <summary>
    /// Draws triangle lists of <see cref="UnlitScene"/>-layout vertices (pixel positions with depth in z, RGBA8 colour,
    /// texture coordinate), each with its texture, onto a cleared colour and depth target of the given size.
    /// </summary>
    public static DrawnImage Draw(GpuDevice gpu, SamplePipeline pipeline, TranslatedShader vertex, TranslatedShader pixel, IReadOnlyList<DrawBatch> batches, int width, int height)
    {
        var device = gpu.Device;
        var targets = CacheStateMapping.Targets(pipeline.State);
        using var root = BindlessRootSignature.Create(device);
        using var state = CreatePipeline(gpu, root, pipeline, vertex, pixel);

        using var context = new CommandContext(device);
        using var arena = new ResourceArena(device);
        using var heaps = new BindlessHeaps(device, UnlitBindings.ResourceSlots(batches.Count), FiveStar.Shaders.Translation.StaticSampler.FirstSlot + FiveStar.Shaders.Translation.StaticSampler.Slots);
        using var target = new RenderTarget(device, targets[0], width, height);
        using var depth = new DepthTarget(device, width, height, Depth(pipeline));

        var list = context.List;
        Bind(heaps, arena, list, width, height, batches);
        StaticSamplers(heaps, pixel);
        heaps.Bind(list);
        list.SetGraphicsRootSignature(root);
        list.SetPipelineState(state);

        target.Begin(list, Clear, depth);
        list.IASetPrimitiveTopology(PrimitiveTopology.TriangleList);

        for (var table = 0; table < batches.Count; table++)
        {
            DrawBatch(list, arena, batches[table], table);
        }

        target.CopyOut(list);
        context.ExecuteAndWait();

        return new DrawnImage(width, height, target.ReadPixels(), gpu.TakeErrors());
    }

    private static void DrawBatch(ID3D12GraphicsCommandList list, ResourceArena arena, DrawBatch batch, int table)
    {
        if (batch.VertexCount == 0)
        {
            return;
        }

        var userData = arena.UploadBuffer(UnlitBindings.UserData(table));
        var vertices = arena.UploadBuffer(batch.Vertices);
        list.SetGraphicsRootConstantBufferView(BindlessRootSignature.UserDataParameter, userData.GPUVirtualAddress);
        list.IASetVertexBuffers(0, new VertexBufferView(vertices.GPUVirtualAddress, (uint)batch.Vertices.Length, UnlitScene.VertexStride));
        list.DrawInstanced((uint)batch.VertexCount, 1, 0, 0);
    }

    /// <summary>The pass's pipeline state; a rejection reports the debug layer's messages when it runs.</summary>
    private static ID3D12PipelineState CreatePipeline(GpuDevice gpu, ID3D12RootSignature root, SamplePipeline pipeline, TranslatedShader vertex, TranslatedShader pixel)
    {
        var layout = CacheStateMapping.InputLayout(pipeline.InputLayout);
        var targets = CacheStateMapping.Targets(pipeline.State);
        var program = new GraphicsProgram(vertex.Dxil, null, pixel.Dxil, layout, targets, DepthTarget.DepthFormat, PrimitiveTopologyType.Triangle);

        try
        {
            return new PipelineBuilder(gpu.Device, root).Graphics(program, CacheStateMapping.DepthTested(pipeline.State).State);
        }
        catch (SharpGen.Runtime.SharpGenException exception)
        {
            var messages = string.Join(Environment.NewLine, gpu.TakeErrors());

            throw new InvalidOperationException($"the pipeline state was rejected ({exception.ResultCode}): {messages}", exception);
        }
    }

    /// <summary>Fills the heap slots the descriptor tables name: the window, the three constant buffers and one texture per batch.</summary>
    private static void Bind(BindlessHeaps heaps, ResourceArena arena, ID3D12GraphicsCommandList list, int width, int height, IReadOnlyList<DrawBatch> batches)
    {
        heaps.RawBuffer(arena.UploadBuffer(UnlitBindings.Window(batches.Count)), UnlitBindings.WindowBytes(batches.Count), UnlitBindings.WindowSlot);
        heaps.RawBuffer(arena.UploadBuffer(UnlitBindings.PixelConstants(AlphaScale, ColourScale)), UnlitBindings.PixelConstantsBytes, UnlitBindings.PixelConstantsSlot);
        heaps.RawBuffer(arena.UploadBuffer(UnlitBindings.ObjectConstants()), UnlitBindings.ObjectConstantsBytes, UnlitBindings.ObjectConstantsSlot);
        heaps.RawBuffer(arena.UploadBuffer(UnlitBindings.ViewConstants(width, height)), UnlitBindings.ViewConstantsBytes, UnlitBindings.ViewConstantsSlot);

        for (var table = 0; table < batches.Count; table++)
        {
            heaps.Texture(arena.Texture2D(list, batches[table].Texture), UnlitBindings.TextureSlot + table);
        }
    }

    /// <summary>Creates the samplers the pixel program builds from literal S# words, each at its slot (StaticSampler).</summary>
    private static void StaticSamplers(BindlessHeaps heaps, TranslatedShader pixel)
    {
        foreach (var sampler in FiveStar.Shaders.Translation.StaticSampler.Read(pixel.Hlsl))
        {
            var desc = FiveStar.Shaders.Translation.DesktopSampler.Describe(sampler.Words, sampler.Comparison);
            heaps.Sampler(System.Runtime.InteropServices.MemoryMarshal.Cast<uint, Vortice.Direct3D12.SamplerDescription>(desc)[0], sampler.Slot);
        }
    }
}
