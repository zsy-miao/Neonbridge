// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.Json;
using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Containers;

namespace FiveStar.Cli.Shader;

/// <summary>Writes the pipeline records of the cache as JSON, with each shader's extracted program names.</summary>
internal static class PipelineCacheJson
{
    public static void Write(string path, PipelineCache cache, CacheProgramMap programs)
    {
        using var stream = File.Create(path);
        using var json = new Utf8JsonWriter(stream, new JsonWriterOptions { Indented = true });
        json.WriteStartObject();
        WriteHeader(json, cache.Header);

        json.WriteStartArray("graphics");

        foreach (var pipeline in cache.Graphics)
        {
            WriteGraphics(json, cache, pipeline, programs);
        }

        json.WriteEndArray();
        json.WriteStartArray("compute");

        foreach (var pipeline in cache.Compute)
        {
            WriteCompute(json, pipeline, ComputeStateParser.NumThreads(cache.Meta(pipeline.StateMeta).Span), programs);
        }

        json.WriteEndArray();
        WriteShaders(json, cache, programs);
        json.WriteEndObject();
    }

    private static void WriteHeader(Utf8JsonWriter json, PipelineCacheHeader header)
    {
        json.WriteString("cacheId", header.CacheId);
        json.WriteString("driverVersion", header.DriverVersion);
        json.WriteNumber("graphicsCount", header.GraphicsCount);
        json.WriteNumber("computeCount", header.ComputeCount);
        json.WriteNumber("metaCount", header.MetaCount);
        json.WriteNumber("shaderCount", header.ShaderCount);
    }

    private static void WriteGraphics(Utf8JsonWriter json, PipelineCache cache, GraphicsPipelineRecord pipeline, CacheProgramMap programs)
    {
        json.WriteStartObject();
        json.WriteNumber("index", pipeline.Index);
        json.WriteString("hash", Hex(pipeline.Hash));
        json.WriteString("key", pipeline.Key);

        json.WriteString("groupA", pipeline.GroupA);
        json.WriteString("groupB", pipeline.GroupB);
        json.WriteNumber("stateMeta", pipeline.StateMeta);
        FixedStateJson.Write(json, cache.State(pipeline));

        var layout = cache.Meta(pipeline.InputLayoutMeta).Span;
        WriteTargets(json, PipelineTargetsParser.Parse(layout));
        WriteInputLayout(json, InputLayoutParser.Parse(layout));
        json.WriteStartArray("stages");

        foreach (var link in pipeline.Stages)
        {
            json.WriteStartObject();
            json.WriteString("slot", link.Slot.ToString().ToLowerInvariant());
            json.WriteNumber("shader", link.Shader);
            WriteNames(json, programs.Programs(link.Shader));
            json.WriteEndObject();
        }

        json.WriteEndArray();
        json.WriteEndObject();
    }

    private static void WriteInputLayout(Utf8JsonWriter json, IReadOnlyList<InputElement> elements)
    {
        json.WriteStartArray("inputLayout");

        foreach (var element in elements)
        {
            json.WriteStartObject();
            json.WriteString("semantic", element.Semantic);
            json.WriteNumber("index", element.SemanticIndex);
            json.WriteNumber("format", element.Format);

            json.WriteNumber("slot", element.InputSlot);
            json.WriteNumber("offset", element.Offset);
            json.WriteNumber("classification", element.Classification);
            json.WriteNumber("stepRate", element.StepRate);
            json.WriteEndObject();
        }

        json.WriteEndArray();
    }

    private static void WriteCompute(Utf8JsonWriter json, ComputePipelineRecord pipeline, ThreadGroupSize threads, CacheProgramMap programs)
    {
        json.WriteStartObject();
        json.WriteNumber("index", pipeline.Index);
        json.WriteString("hash", Hex(pipeline.Hash));
        json.WriteNumber("stateMeta", pipeline.StateMeta);

        json.WriteStartArray("numThreads");
        json.WriteNumberValue(threads.X);
        json.WriteNumberValue(threads.Y);
        json.WriteNumberValue(threads.Z);
        json.WriteEndArray();

        json.WriteNumber("shader", pipeline.Shader);
        WriteNames(json, programs.Programs(pipeline.Shader));
        json.WriteEndObject();
    }

    // One entry per shader: its programs and the entries of its metadata (each shader has one metadata entry).
    private static void WriteShaders(Utf8JsonWriter json, PipelineCache cache, CacheProgramMap programs)
    {
        var metas = new int[cache.Header.ShaderCount];
        Array.Fill(metas, -1);

        foreach (var link in cache.Graphics.SelectMany(pipeline => pipeline.Stages))
        {
            metas[link.Shader] = link.Meta;
        }

        foreach (var pipeline in cache.Compute)
        {
            metas[pipeline.Shader] = pipeline.ShaderMeta;
        }

        json.WriteStartArray("shaders");

        for (var shader = 0; shader < metas.Length; shader++)
        {
            json.WriteStartObject();
            json.WriteNumber("index", shader);
            json.WriteBoolean("available", cache.Shader(shader).HasValue);
            WriteNames(json, programs.Programs(shader));
            WriteMeta(json, cache, metas[shader]);
            json.WriteEndObject();
        }

        json.WriteEndArray();
    }

    private static void WriteMeta(Utf8JsonWriter json, PipelineCache cache, int index)
    {
        if (index < 0)
        {
            return;
        }

        var meta = ShaderMetaParser.Parse(cache.Meta(index).Span);

        if (meta is null)
        {
            return;
        }

        json.WriteString("hash", meta.Hash);
        json.WriteStartArray("userData");

        foreach (var entry in meta.UserData)
        {
            json.WriteStartObject();
            json.WriteString("kind", entry.Kind.ToString());
            json.WriteString("class", entry.RegisterClass);
            json.WriteNumber("register", entry.Register);
            json.WriteNumber("parameter", entry.Parameter);

            json.WriteNumber("sgpr", entry.Sgpr);
            json.WriteNumber("tableOffset", entry.TableOffset);
            json.WriteNumber("size", entry.Size);
            json.WriteNumber("stride", entry.Stride);
            json.WriteEndObject();
        }

        json.WriteEndArray();
        WriteResources(json, meta.Resources);
    }

    private static void WriteResources(Utf8JsonWriter json, IReadOnlyList<ResourceEntry> resources)
    {
        json.WriteStartArray("resources");

        foreach (var entry in resources)
        {
            json.WriteStartObject();
            json.WriteBoolean("used", entry.Used);
            json.WriteString("class", entry.Class.ToString());
            json.WriteNumber("kind", entry.ResourceKind);

            json.WriteNumber("register", entry.Register);
            json.WriteNumber("space", entry.Space);
            json.WriteEndObject();
        }

        json.WriteEndArray();
    }

    private static void WriteTargets(Utf8JsonWriter json, PipelineTargets targets)
    {
        json.WriteStartObject("targets");
        json.WriteStartArray("renderTargetFormats");

        foreach (var format in targets.RenderTargetFormats)
        {
            json.WriteNumberValue(format);
        }

        json.WriteEndArray();
        json.WriteNumber("depthStencilFormat", targets.DepthStencilFormat);
        json.WriteNumber("topologyType", targets.TopologyType);
        json.WriteNumber("sampleCount", targets.SampleCount);

        json.WriteString("sampleMask", Hex(targets.SampleMask));
        json.WriteEndObject();
    }

    private static void WriteNames(Utf8JsonWriter json, IReadOnlyList<string> names)
    {
        json.WriteStartArray("programs");

        foreach (var name in names)
        {
            json.WriteStringValue(name);
        }

        json.WriteEndArray();
    }

    private static string Hex(ulong value)
    {
        return "0x" + value.ToString("x", CultureInfo.InvariantCulture);
    }
}
