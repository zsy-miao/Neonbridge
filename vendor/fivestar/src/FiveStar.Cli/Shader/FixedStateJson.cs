// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.Json;
using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Shader;

/// <summary>Writes a graphics pipeline's fixed-function state: D3D12 enum values, then the raw registers.</summary>
internal static class FixedStateJson
{
    public static void Write(Utf8JsonWriter json, GraphicsStateRegisters? registers)
    {
        if (registers is null)
        {
            json.WriteNull("state");

            return;
        }

        var state = FixedFunctionDecoder.Decode(registers);
        json.WriteStartObject("state");
        json.WriteNumber("topologyType", state.TopologyType);
        json.WriteNumber("sampleCount", FixedFunctionState.SampleCount);
        json.WriteNumber("forcedSampleCount", state.ForcedSampleCount);
        json.WriteBoolean("dualSource", state.DualSource);

        WriteTargets(json, state);
        WriteBlend(json, state.Blend);
        WriteDepthStencil(json, state.DepthStencil);
        WriteRasterizer(json, state.Rasterizer);
        WriteRegisters(json, registers);
        json.WriteEndObject();
    }

    private static void WriteTargets(Utf8JsonWriter json, FixedFunctionState state)
    {
        json.WriteNumber("renderTargetCount", state.RenderTargetCount);
        json.WriteStartArray("renderTargets");

        foreach (var target in state.RenderTargets)
        {
            json.WriteStartObject();
            json.WriteNumber("index", target.Index);
            json.WriteNumber("formatGuess", target.Format);
            json.WriteNumber("downconvert", target.Downconvert);
            json.WriteNumber("exportFormat", target.ExportFormat);
            json.WriteEndObject();
        }

        json.WriteEndArray();
    }

    private static void WriteBlend(Utf8JsonWriter json, IReadOnlyList<BlendTarget> blend)
    {
        json.WriteStartArray("blend");

        foreach (var target in blend)
        {
            json.WriteStartObject();
            json.WriteBoolean("enable", target.Enable);
            json.WriteNumber("src", target.SrcBlend);
            json.WriteNumber("dest", target.DestBlend);
            json.WriteNumber("op", target.BlendOp);

            json.WriteNumber("srcAlpha", target.SrcBlendAlpha);
            json.WriteNumber("destAlpha", target.DestBlendAlpha);
            json.WriteNumber("opAlpha", target.BlendOpAlpha);
            json.WriteNumber("writeMask", target.WriteMask);
            json.WriteEndObject();
        }

        json.WriteEndArray();
    }

    private static void WriteDepthStencil(Utf8JsonWriter json, DepthStencilState depth)
    {
        json.WriteStartObject("depthStencil");
        json.WriteBoolean("depthEnable", depth.DepthEnable);
        json.WriteBoolean("depthWrite", depth.DepthWrite);
        json.WriteNumber("depthFunc", depth.DepthFunc);
        json.WriteBoolean("depthBoundsTest", depth.DepthBoundsTest);

        json.WriteBoolean("stencilEnable", depth.StencilEnable);
        json.WriteNumber("stencilReadMask", depth.StencilReadMask);
        json.WriteNumber("stencilWriteMask", depth.StencilWriteMask);
        WriteFace(json, "front", depth.Front);
        WriteFace(json, "back", depth.Back);
        json.WriteEndObject();
    }

    private static void WriteFace(Utf8JsonWriter json, string name, StencilFace face)
    {
        json.WriteStartObject(name);
        json.WriteNumber("failOp", face.FailOp);
        json.WriteNumber("depthFailOp", face.DepthFailOp);
        json.WriteNumber("passOp", face.PassOp);
        json.WriteNumber("func", face.Func);
        json.WriteEndObject();
    }

    private static void WriteRasterizer(Utf8JsonWriter json, RasterizerState rasterizer)
    {
        json.WriteStartObject("rasterizer");
        json.WriteNumber("fillMode", rasterizer.FillMode);
        json.WriteNumber("cullMode", rasterizer.CullMode);
        json.WriteBoolean("frontCounterClockwise", rasterizer.FrontCounterClockwise);

        json.WriteNumber("depthBias", rasterizer.DepthBias);
        json.WriteNumber("depthBiasClamp", rasterizer.DepthBiasClamp);
        json.WriteNumber("slopeScaledDepthBias", rasterizer.SlopeScaledDepthBias);
        json.WriteBoolean("depthClipEnable", rasterizer.DepthClipEnable);
        json.WriteEndObject();
    }

    private static void WriteRegisters(Utf8JsonWriter json, GraphicsStateRegisters registers)
    {
        json.WriteStartObject("registers");
        json.WriteString("DB_DEPTH_CONTROL", Hex(registers.DepthControl));
        json.WriteString("DB_STENCIL_CONTROL", Hex(registers.StencilControl));
        json.WriteString("DB_STENCILREFMASK", Hex(registers.StencilRefMask));
        json.WriteString("DB_STENCILREFMASK_BF", Hex(registers.StencilRefMaskBack));
        json.WriteString("DB_EQAA", Hex(registers.Eqaa));

        json.WriteString("PA_SU_SC_MODE_CNTL", Hex(registers.ModeControl));
        json.WriteString("PA_CL_CLIP_CNTL", Hex(registers.ClipControl));
        json.WriteString("VGT_GS_OUT_PRIM_TYPE", Hex(registers.GsOutPrimType));
        json.WriteString("SPI_SHADER_COL_FORMAT", Hex(registers.ColFormat));
        json.WriteString("CB_SHADER_MASK", Hex(registers.ShaderMask));
        json.WriteString("CB_TARGET_MASK", Hex(registers.TargetMask));

        json.WriteString("SX_PS_DOWNCONVERT", Hex(registers.Downconvert));
        json.WriteString("SX_BLEND_OPT_EPSILON", Hex(registers.BlendOptEpsilon));
        json.WriteString("SX_BLEND_OPT_CONTROL", Hex(registers.BlendOptControl));
        json.WriteStartArray("CB_BLEND_CONTROL");

        foreach (var control in registers.BlendControl)
        {
            json.WriteStringValue(Hex(control));
        }

        json.WriteEndArray();
        json.WriteEndObject();
    }

    private static string Hex(uint value)
    {
        return "0x" + value.ToString("x8", CultureInfo.InvariantCulture);
    }
}
