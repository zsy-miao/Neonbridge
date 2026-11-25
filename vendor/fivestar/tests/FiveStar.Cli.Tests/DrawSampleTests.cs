// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Graphics;
using FiveStar.Cli.Tests.Support;
using FiveStar.Graphics;
using FiveStar.Shaders.Disassembly;
using Vortice.DXGI;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>
/// The game's immediate-mode unlit pass, translated, draws a real uncompressed texture on WARP with the pipeline
/// cache's state: one texel per pixel, alpha-blended over the clear colour.
/// </summary>
public sealed class DrawSampleTests
{
    // An R8G8B8A8_UNORM texture of the reference dump (256x256), so the expected pixels follow from its bytes.
    private const string TexturePath = @"data\ttdb\graphics\ttdb_graphics.ttpf";
    private const int TextureIndex = 234;
    private const int Tolerance = 2;

    [RenderFact]
    public void The_translated_unlit_pass_draws_a_game_texture_texel_for_texel()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-draw-");

        try
        {
            var inputs = SampleInputs.Load(
                DumpFactAttribute.Folder!, DumpFactAttribute.ShaderFolder!, Path.Combine(DumpFactAttribute.Folder!, TexturePath), TextureIndex,
                work.FullName, new RdnaDecoder(), RenderFactAttribute.Dxc);
            using var device = GpuDevice.CreateWarp(debugLayer: false);

            var frame = UnlitRenderer.Render(device, inputs.Pipeline, inputs.Vertex, inputs.Pixel, inputs.Texture);

            Assert.Equal((0, Format.R8G8B8A8_UNorm), (inputs.Pipeline.Record.Index, inputs.Texture.Format));
            Assert.Equal((true, 5, 6, 3), (inputs.Pipeline.State.Blend[0].Enable, inputs.Pipeline.State.Blend[0].SrcBlend, inputs.Pipeline.State.Blend[0].DestBlend, inputs.Pipeline.State.Rasterizer.CullMode));
            Assert.Equal([32, 32, 32, 255], frame.Pixel(0, 0).ToArray());
            Assert.Empty(Mismatches(frame, inputs.Texture.Data));

            // The strip below: the vertex colour runs from red on the left to blue on the right.
            var middle = frame.Scene.StripTop + (frame.Scene.StripHeight / 2);
            var (left, right) = (frame.Pixel(UnlitScene.Margin + 1, middle).ToArray(), frame.Pixel(frame.Scene.Width - UnlitScene.Margin - 2, middle).ToArray());
            Assert.True(right[2] > left[2] && left[0] >= right[0], $"strip left {string.Join(',', left)}, right {string.Join(',', right)}");
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    [RenderFact]
    public void The_depth_test_keeps_the_nearer_of_two_overlapping_quads_whichever_comes_first()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-depth-");

        try
        {
            var pass = UnlitPass.Load(DumpFactAttribute.Folder!, DumpFactAttribute.ShaderFolder!, work.FullName, new RdnaDecoder(), RenderFactAttribute.Dxc);
            var depth = UnlitRenderer.Depth(pass.Pipeline);
            using var device = GpuDevice.CreateWarp(debugLayer: false);

            // Near red quad on the left, then a far blue quad on the right; they overlap in x 24..40.
            DrawBatch[] batches =
            [
                new(ModelTexture.Solid(0xFF0000), Quad(8, 40, depth.Depth(0.25f))),
                new(ModelTexture.Solid(0x0000FF), Quad(24, 56, depth.Depth(0.75f))),
            ];
            var image = UnlitRenderer.Draw(device, pass.Pipeline, pass.Vertex, pass.Pixel, batches, 64, 64);

            Assert.True(depth.Reversed);
            Assert.Equal([255, 0, 0, 255], Pixel(image, 32, 32));
            Assert.Equal([0, 0, 255, 255], Pixel(image, 48, 32));
            Assert.Empty(image.Errors);
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    /// <summary>A white quad from x = left to right, y 8..56, at one depth, wound like <see cref="UnlitScene"/>.</summary>
    private static byte[] Quad(float left, float right, float depth)
    {
        (float X, float Y, float U, float V)[] corners = [(left, 8, 0, 0), (left, 56, 0, 1), (right, 56, 1, 1), (left, 8, 0, 0), (right, 56, 1, 1), (right, 8, 1, 0)];
        var data = new byte[corners.Length * UnlitScene.VertexStride];

        for (var index = 0; index < corners.Length; index++)
        {
            var vertex = data.AsSpan(index * UnlitScene.VertexStride);
            BitConverter.TryWriteBytes(vertex, corners[index].X);
            BitConverter.TryWriteBytes(vertex[4..], corners[index].Y);
            BitConverter.TryWriteBytes(vertex[8..], depth);

            BitConverter.TryWriteBytes(vertex[12..], UnlitScene.White);
            BitConverter.TryWriteBytes(vertex[16..], corners[index].U);
            BitConverter.TryWriteBytes(vertex[20..], corners[index].V);
        }

        return data;
    }

    private static byte[] Pixel(DrawnImage image, int x, int y)
    {
        return image.Pixels.AsSpan(((y * image.Width) + x) * 4, 4).ToArray();
    }

    /// <summary>Pixels of the texture quad that differ from texel over clear (SRC_ALPHA, INV_SRC_ALPHA) by more than the tolerance.</summary>
    private static List<string> Mismatches(SampleFrame frame, byte[] texels)
    {
        var mismatches = new List<string>();
        var size = frame.Scene.TextureWidth;
        var clear = frame.Pixel(0, 0).ToArray();

        for (var y = 0; y < frame.Scene.TextureHeight; y++)
        {
            for (var x = 0; x < size; x++)
            {
                var texel = texels.AsSpan(((y * size) + x) * 4, 4);
                var actual = frame.Pixel(UnlitScene.Margin + x, UnlitScene.Margin + y);
                var alpha = texel[3] / 255.0;

                for (var channel = 0; channel < 4; channel++)
                {
                    // Blend: texel * alpha + clear * (1 - alpha), the alpha channel included.
                    var expected = (texel[channel] / 255.0 * alpha) + (clear[channel] / 255.0 * (1 - alpha));

                    if (Math.Abs(actual[channel] - (expected * 255)) > Tolerance)
                    {
                        mismatches.Add($"({x},{y}) channel {channel}: expected {expected * 255:F1}, got {actual[channel]}");
                    }
                }
            }
        }

        return mismatches.Take(10).ToList();
    }
}
