// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;
using SharpGen.Runtime;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>A producer linked to its pixel shader's interpolation modes creates a pipeline and shades as the pair should.</summary>
public sealed class StageLinkerTests
{
    // A vertex shader as the translator writes it: one parameter register holding two float2 elements, the second
    // different per vertex. Without modes dxc packs both into one register.
    private const string Producer = """
        struct Out {
          float2 p0_xy : TEXCOORD0;
          float2 p0_zw : TEXCOORD1;
          float4 pos0 : SV_Position;
        };
        Out main(uint vid : SV_VertexID) {
          Out o;
          float2 uv = float2((vid << 1) & 2, vid & 2);
          o.p0_xy = float2(1.5, 2.5);
          o.p0_zw = float2(vid * 10 + 1, vid * 10 + 2);
          o.pos0 = float4(uv * float2(1, -1) + float2(-1, 1), 0, 1);
          return o;
        }
        """;

    // The pixel shader reads the second element flat, so its own packing gives it a register of its own.
    private const string Consumer = """
        struct PSIn {
          float2 a0_xy : TEXCOORD0;
          nointerpolation float2 a0_zw : TEXCOORD1;
        };
        uint4 main(PSIn pin) : SV_Target0 {
          return uint4(asuint(pin.a0_xy), asuint(pin.a0_zw));
        }
        """;

    private const int Size = 8;

    // Linear (1.5, 2.5) and the flat value of vertex 0.
    private static readonly float[] Expected = [1.5f, 2.5f, 1f, 2f];

    [WarpFact]
    public void Linked_producer_creates_the_pipeline_and_passes_the_flat_value_of_the_provoking_vertex()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-link").FullName;
        var workbench = new ShaderWorkbench(ShaderToolchain.Find(), folder);
        var pixel = workbench.Compile("ps", Consumer, "ps");
        var inputs = PsvSignatureParser.Parse(DxbcContainer.Parse(pixel).Find(PsvSignatureParser.ChunkCode)!.Data.Span).Inputs;
        using var device = new WarpDevice();
        var runner = new WarpRunner(device, UserDataBinding.RootConstants);

        var unlinked = workbench.Compile("vs", Producer, "vs");
        Assert.ThrowsAny<SharpGenException>(() => runner.Graphics(Job(unlinked, pixel)));

        var linked = workbench.Compile("vs_linked", StageLinker.Link(Producer, inputs), "vs");
        var result = runner.Graphics(Job(linked, pixel));

        Assert.Equal(Expected, result.Pixel(0, Size / 2, Size / 2).Select(BitConverter.UInt32BitsToSingle));
    }

    [Fact]
    public void A_pixel_input_the_producer_lacks_or_moves_needs_a_link()
    {
        PsvElement[] inputs = [Element("TEXCOORD", 0, row: 0), Element("ATTRLO", 3, row: 1)];
        PsvElement[] outputs = [Element("TEXCOORD", 0, row: 0), Element("COLOR", 0, row: 1)];

        Assert.False(StageLinker.Links(outputs, inputs));
        Assert.True(StageLinker.Links([.. outputs, Element("ATTRLO", 3, row: 1)], inputs));
        Assert.False(StageLinker.Links([Element("TEXCOORD", 0, row: 2), Element("ATTRLO", 3, row: 1)], inputs));
    }

    [Fact]
    public void Trimming_keeps_only_the_inputs_the_pixel_shader_reads()
    {
        const string pixel = """
            struct PSIn {
              float4 a0_xyzw : TEXCOORD0;
              float3 unused1_xyz : TEXCOORD1;
              float4 attrlo0 : ATTRLO0;
              float4 attrhi0 : ATTRHI0;
              float4 attrlo1 : ATTRLO1;
              bool front : SV_IsFrontFace;
            };
            float4 main(PSIn pin) : SV_Target0 {
              float4 attr0 = pin.a0_xyzw;
              float4 attrlo1 = pin.attrlo1;
              return attr0 + attrlo1;
            }
            """;

        var trimmed = StageLinker.TrimInputs(pixel);

        Assert.Contains("a0_xyzw : TEXCOORD0", trimmed, StringComparison.Ordinal);
        Assert.Contains("attrlo1 : ATTRLO1", trimmed, StringComparison.Ordinal);
        Assert.Contains("front : SV_IsFrontFace", trimmed, StringComparison.Ordinal);
        Assert.DoesNotContain("unused1_xyz", trimmed, StringComparison.Ordinal);
        Assert.DoesNotContain("ATTRLO0", trimmed, StringComparison.Ordinal);
        Assert.DoesNotContain("ATTRHI0", trimmed, StringComparison.Ordinal);
    }

    [Fact]
    public void Outputs_the_pixel_shader_does_not_read_can_leave_the_out_struct()
    {
        const string producer = """
            struct Out {
              float4 p0_xyzw : TEXCOORD0;
              float4 p1_xyzw : TEXCOORD1;
              float4 pos0 : SV_Position;
            };
            Out main() {
              Out o;
              o.p0_xyzw = 1;
              o.p1_xyzw = 2;
              o.pos0 = 0;
              return o;
            }
            """;

        var linked = StageLinker.Link(producer, [Element("TEXCOORD", 0, row: 0)], dropUnread: true);

        Assert.DoesNotContain("p1_xyzw : TEXCOORD1", linked, StringComparison.Ordinal);
        Assert.Contains("static float4 unread_p1_xyzw;", linked, StringComparison.Ordinal);
        Assert.Contains("unread_p1_xyzw = 2;", linked, StringComparison.Ordinal);
        Assert.Contains("pos0 : SV_Position", linked, StringComparison.Ordinal);
    }

    private static PsvElement Element(string name, int index, int row)
    {
        return new PsvElement(name, index, 1, row, 4, 0, true, PsvSemanticKind.Arbitrary, 3, 2);
    }

    private static GraphicsJob Job(byte[] vertex, byte[] pixel)
    {
        return new GraphicsJob(vertex, pixel, Framebuffer.Fullscreen(1, Size, Size, isFloat: false), new Bindings());
    }
}
