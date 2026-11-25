// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>A generated pass-through VS writes every PS input to the register and components the PS reads it from.</summary>
public sealed class PassThroughVertexShaderTests
{
    private const string PixelShader = """
        struct PSIn {
            float2 uv : TEXCOORD0;
            nointerpolation uint id : TEXCOORD1;
            centroid float3 normal : NORMAL0;
            noperspective float depth : TEXCOORD2;
            float4 rows[2] : COLOR3;
        };
        float4 main(PSIn pin, float4 position : SV_Position, bool front : SV_IsFrontFace) : SV_Target0 {
            return float4(pin.uv, pin.normal.x + pin.depth, (float)pin.id + position.x + front + pin.rows[1].y);
        }
        """;

    [WarpFact]
    public void Outputs_land_in_the_pixel_shader_input_registers()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-passthrough").FullName;
        var workbench = new ShaderWorkbench(ShaderToolchain.Find(), folder);
        var pixel = workbench.Compile("ps", PixelShader, "ps");
        var psv = DxbcContainer.Parse(pixel).Find(PsvSignatureParser.ChunkCode)!;

        var vertex = workbench.Compile("vs", PassThroughVertexShader.Source(PsvSignatureParser.Parse(psv.Data.Span).Inputs, needsPosition: true), "vs");

        var reads = Signature(pixel, "ISG1").Where(element => element.SystemValue != "SV_IsFrontFace");
        var writes = Signature(vertex, "OSG1");
        Assert.All(reads, read => Assert.Contains(writes, write => (write.Name, write.SemanticIndex, write.Register, write.Mask) == (read.Name, read.SemanticIndex, read.Register, read.Mask)));
    }

    private static IReadOnlyList<SignatureElement> Signature(byte[] program, string chunk)
    {
        return SignatureParser.Parse(DxbcContainer.Parse(program).Find(chunk)!.Data.Span);
    }
}
