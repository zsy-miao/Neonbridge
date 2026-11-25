// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Pipelines;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The journal of the pipelines the game asked for: kept across boots in first-use order, each description once, read
/// back as warm-up targets that make the same desktop pipeline.
/// </summary>
public sealed class D3D12XPipelineJournalTests
{
    [Fact]
    public void Descriptions_come_back_once_each_in_first_use_order_across_boots()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-journal-");

        try
        {
            var graphics = Graphics();
            var compute = new ComputePipelineDescription(0x1234, Shader(7));

            using (var boot = PipelineJournal.Open(folder.FullName))
            {
                boot.Record(graphics);
                boot.Record(graphics with { GameRootSignature = 0x5678 });
            }

            using (var boot = PipelineJournal.Open(folder.FullName))
            {
                boot.Record(compute);
                boot.Record(graphics);
                Assert.Equal(2, boot.Count);
            }

            var targets = PipelineJournal.Read(folder.FullName);

            Assert.Equal(2, targets.Count);
            var read = targets[0].Graphics!;
            Assert.Equal(graphics.Shaders.Select(shader => shader.Bytes), read.Shaders.Select(shader => shader.Bytes));
            Assert.Equal(graphics.Blend, read.Blend);

            Assert.Equal(graphics.InputLayout, read.InputLayout);
            Assert.Equal(graphics.RenderTargetFormats, read.RenderTargetFormats);
            Assert.Equal(graphics.DepthStencilFormat, read.DepthStencilFormat);
            Assert.Equal(compute.Shader.Bytes, targets[1].Compute!.Shader.Bytes);
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    private static GraphicsPipelineDescription Graphics()
    {
        return GraphicsPipelineDescription.Defaults(0x1234) with
        {
            Shaders = [Shader(1), Shader(2)],
            InputLayout = [new VertexElement("POSITION", 0, 6, 0, 0, 0, 0)],
            RenderTargetFormats = [28, 10],
            DepthStencilFormat = 20,
        };
    }

    private static ShaderSource Shader(byte seed)
    {
        return new ShaderSource(ShaderSourceKind.CacheCode, Enumerable.Repeat(seed, 24).ToArray());
    }
}
