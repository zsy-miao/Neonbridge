// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using Vortice.Direct3D12;
using Vortice.DXGI;
using Vortice.Mathematics;
using Xunit;
using DescriptorHeapDescription = Vortice.Direct3D12.DescriptorHeapDescription;
using ResourceDescription = Vortice.Direct3D12.ResourceDescription;
using TranslatedProgram = FiveStar.Loader.Shims.D3D12X.Pipelines.TranslatedProgram;

namespace FiveStar.Loader.Tests;

/// <summary>
/// A console rectangle-list draw through the D3D12.X slots (25 SetPipelineState, 20 topology 17, 12 DrawInstanced) on
/// the default adapter: three vertices cover the whole rectangle, and the fourth corner's colour is extrapolated.
/// </summary>
public sealed unsafe class D3D12XRectangleListTests
{
    private const int Size = 64;
    private const int BytesPerPixel = 4;
    private const int SetPipelineSlot = 25;
    private const int SetTopologySlot = 20;
    private const int DrawSlot = 12;
    private const int CloseSlot = 9;
    private const int QueueExecuteSlot = 35;
    private const int QueueSignalSlot = 16;
    private const int FenceWaitSlot = 9;
    private const uint RectangleList = 17;

    /// <summary>A rectangle over the middle half of the target: corner, then its neighbours in clockwise order; colour = position + 0.5.</summary>
    private const string Vertex = """
        struct Out { float4 color : COLOR0; float4 position : SV_Position; };
        Out main(uint id : SV_VertexID)
        {
            const float2 corners[3] = { float2(-0.5, -0.5), float2(-0.5, 0.5), float2(0.5, -0.5) };
            Out o;
            o.position = float4(corners[id], 0, 1);
            o.color = float4(corners[id] + 0.5, 0, 1);
            return o;
        }
        """;

    private const string Pixel = "float4 main(float4 color : COLOR0) : SV_Target { return color; }";

    [DxcFact]
    public void A_rectangle_list_draw_covers_the_whole_rectangle()
    {
        CoversTheWholeRectangle("vs");
    }

    [DxcFact]
    public void A_rectangle_list_draw_of_a_linked_producer_the_library_cannot_reload_covers_the_whole_rectangle()
    {
        // VS_TiledLighting linked to its TL_PS: the library reads no ".link-" program by name, so the recipe keeps it.
        CoversTheWholeRectangle("vs.link-beb72bb57485");
    }

    private static void CoversTheWholeRectangle(string vertexName)
    {
        using var test = TestRuntime.Create();
        var device = new XboxDevice(test);
        var folder = Directory.CreateTempSubdirectory("fivestar-rect-");

        try
        {
            var desktop = new ID3D12Device(test.Runtime.D3D12X.Objects.DesktopDevice);
            desktop.AddRef();

            using (desktop)
            {
                nint pipeline = Pipelines(test, folder.FullName, vertexName);
                var pixels = Draw(device, desktop, pipeline, test.Runtime.D3D12X.Bindings.Root);

                // The middle half is covered (all four quarters, the extrapolated one included), the border is not.
                Assert.All(new[] { (20, 20), (44, 20), (20, 44), (44, 44) }, p => Assert.NotEqual(0u, pixels[(p.Item2 * Size) + p.Item1]));
                Assert.All(new[] { (4, 4), (60, 32), (32, 60) }, p => Assert.Equal(0u, pixels[(p.Item2 * Size) + p.Item1]));

                // Near the fourth corner (x 0.45, y 0.45) the colour continues the other three: red and green near 0.95.
                uint corner = pixels[(17 * Size) + 46];
                Assert.True((corner & 0xFF) > 220 && ((corner >> 8) & 0xFF) > 220, $"corner colour 0x{corner:x8}");
                Assert.Equal(1, test.Runtime.D3D12X.Rectangles.Expanded);
            }
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    /// <summary>The base pipeline, and its rectangle-list variant's geometry shader compiled as <c>fivestar shader rect-gs</c> does.</summary>
    private static nint Pipelines(TestRuntime test, string folder, string vertexName)
    {
        var vertex = Compile(folder, "vs", Vertex);
        var programs = new List<TranslatedProgram> { new(vertexName, HardwareStage.Vertex, vertex), new("ps", HardwareStage.Pixel, Compile(folder, "ps", Pixel)) };
        var outputs = PsvSignatureParser.Parse(DxbcContainer.Parse(vertex).Find(PsvSignatureParser.ChunkCode)!.Data.Span).Outputs;
        var source = RectangleGeometryShader.Source(outputs);
        File.WriteAllBytes(Path.Combine(folder, RectangleGeometryShader.FileName(source)), Compile(folder, "gs", source));

        nint device2;
        var device2Iid = DesktopDevice.Device2Iid;
        Assert.Equal(0, DesktopDevice.QueryInterface(test.Runtime.D3D12X.Objects.DesktopDevice, &device2Iid, &device2));
        nint root = test.Runtime.D3D12X.Bindings.Root;
        var description = GraphicsPipelineDescription.Defaults(0) with { RenderTargetFormats = [(int)Format.R8G8B8A8_UNorm] };

        var iid = DesktopDevice.PipelineStateIid;
        nint pipeline;
        Assert.Equal(0, DesktopPipelineBuilder.Graphics(device2, root, description, programs, &iid, &pipeline));

        var library = new FixedProgramLibrary([.. programs.Where(program => !RectanglePipelines.Derived(program.Name))]);
        var rectangles = new RectanglePipelines(device2, root, folder, library, new OnceLog(new LoaderLog(new StringWriter(), null, false)));
        rectangles.Remember(pipeline, description, programs);
        test.Runtime.D3D12X.Rectangles.Attach(rectangles);

        return pipeline;
    }

    /// <summary>Clears a target, draws one rectangle through the game slots and reads the target back.</summary>
    private static uint[] Draw(XboxDevice device, ID3D12Device desktop, nint pipeline, nint root)
    {
        using var target = desktop.CreateCommittedResource(
            new HeapProperties(HeapType.Default), HeapFlags.None, ResourceDescription.Texture2D(Format.R8G8B8A8_UNorm, Size, Size, 1, 1, flags: ResourceFlags.AllowRenderTarget), ResourceStates.RenderTarget);
        using var readback = desktop.CreateCommittedResource(new HeapProperties(HeapType.Readback), HeapFlags.None, ResourceDescription.Buffer(Size * Size * BytesPerPixel), ResourceStates.CopyDest);
        using var heap = desktop.CreateDescriptorHeap(new DescriptorHeapDescription(DescriptorHeapType.RenderTargetView, 1));
        var rtv = heap.GetCPUDescriptorHandleForHeapStart();
        desktop.CreateRenderTargetView(target, null, rtv);

        nint list = device.DirectList();
        var inner = new ID3D12GraphicsCommandList(((XboxObject*)list)->Inner);
        inner.AddRef();

        using (inner)
        {
            var signature = new ID3D12RootSignature(root);
            signature.AddRef();
            inner.SetGraphicsRootSignature(signature);

            inner.OMSetRenderTargets(rtv);
            inner.RSSetViewport(new Viewport(Size, Size));
            inner.RSSetScissorRect(Size, Size);
            inner.ClearRenderTargetView(rtv, new Color4(0, 0, 0, 0));

            ((delegate* unmanaged<nint, nint, void>)XboxDevice.Vtable(list)[SetPipelineSlot])(list, pipeline);
            ((delegate* unmanaged<nint, uint, void>)XboxDevice.Vtable(list)[SetTopologySlot])(list, RectangleList);
            ((delegate* unmanaged<nint, uint, uint, uint, uint, void>)XboxDevice.Vtable(list)[DrawSlot])(list, 3, 1, 0, 0);

            inner.ResourceBarrierTransition(target, ResourceStates.RenderTarget, ResourceStates.CopySource);
            var footprint = new PlacedSubresourceFootPrint { Footprint = new SubresourceFootPrint(Format.R8G8B8A8_UNorm, Size, Size, 1, Size * BytesPerPixel) };
            inner.CopyTextureRegion(new TextureCopyLocation(readback, footprint), 0, 0, 0, new TextureCopyLocation(target, 0));
        }

        Run(device, list);

        return Read(readback);
    }

    private static uint[] Read(ID3D12Resource readback)
    {
        var pixels = new uint[Size * Size];
        void* data;
        readback.Map(0, null, &data).CheckError();
        new ReadOnlySpan<uint>(data, pixels.Length).CopyTo(pixels);
        readback.Unmap(0);

        return pixels;
    }

    private static byte[] Compile(string folder, string profile, string source)
    {
        string input = Path.Combine(folder, profile + ".hlsl");
        string output = Path.Combine(folder, profile + ".dxil");
        File.WriteAllText(input, source);

        using var dxc = Process.Start(new ProcessStartInfo(PipelineFactAttribute.Dxc, ["-T", profile + "_6_6", "-E", "main", "-Fo", output, input]) { RedirectStandardError = true })!;
        string errors = dxc.StandardError.ReadToEnd();
        dxc.WaitForExit();
        Assert.True(dxc.ExitCode == 0, errors);

        return File.ReadAllBytes(output);
    }

    /// <summary>Close (list 9), ExecuteCommandListsX (queue 35), Signal (queue 16) and a blocking SetEventOnCompletion (fence 9).</summary>
    private static void Run(XboxDevice device, nint list)
    {
        nint queue = device.Queue();
        nint fence = device.Fence();
        Assert.Equal(0, ((delegate* unmanaged<nint, int>)XboxDevice.Vtable(list)[CloseSlot])(list));

        nint* lists = stackalloc nint[] { list };
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, nint*, nint, uint, int>)XboxDevice.Vtable(queue)[QueueExecuteSlot])(queue, 1, lists, 0, 0));
        Assert.Equal(0, ((delegate* unmanaged<nint, nint, ulong, int>)XboxDevice.Vtable(queue)[QueueSignalSlot])(queue, fence, 1));
        Assert.Equal(0, ((delegate* unmanaged<nint, ulong, nint, int>)XboxDevice.Vtable(fence)[FenceWaitSlot])(fence, 1, 0));
    }
}
