// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The messages between the loader and its compile host (<see cref="CompileHost"/>, <see cref="CompileHostServer"/>)
/// over the host's standard input and output. A request is its id, its kind, the priority of the thread that asks (the
/// host works at the same, so a game thread's inline compile keeps its priority), then for a compile the profile,
/// source path and source, and for a make the program file, the output folder and the adapter facts that pick the form.
/// An answer is the id, its kind (<see cref="HostAnswerKind"/>), a text (why not), and for a make dxc must finish the
/// profile, source file name and source. Strings are length-prefixed UTF-8 (<see cref="BinaryWriter.Write(string)"/>).
/// </summary>
internal static class CompileHostProtocol
{
    /// <summary>The loader's first argument that makes it a compile host; the second, when present, names dxil.dll.</summary>
    public const string Mode = "--compile-host";

    /// <summary>A compile of translated HLSL.</summary>
    public const int CompileKind = 0;

    /// <summary>A make: translate an extracted program, pick the adapter's form and compile it.</summary>
    public const int MakeKind = 1;

    public static void WriteCompile(BinaryWriter writer, int id, ThreadPriority priority, CompileRequest request)
    {
        writer.Write(id);
        writer.Write(CompileKind);
        writer.Write((int)priority);

        writer.Write(request.Profile);
        writer.Write(request.HlslPath);
        writer.Write(request.Hlsl);
        writer.Flush();
    }

    public static void WriteMake(BinaryWriter writer, int id, ThreadPriority priority, MakeRequest request)
    {
        writer.Write(id);
        writer.Write(MakeKind);
        writer.Write((int)priority);

        writer.Write(request.Name);
        writer.Write(request.Programs);
        writer.Write(request.Folder);

        writer.Write(request.Adapter.ShaderModel ?? string.Empty);
        writer.Write(request.Adapter.WaveLaneCountMin);
        writer.Write(request.Adapter.WaveLaneCountMax);
        writer.Write(request.Adapter.MeshShaderTier ?? string.Empty);
        writer.Write(request.Adapter.MeshWaveLanes ?? 0);

        writer.Write(request.Header);
        writer.Flush();
    }

    /// <summary>The next request: id, kind, priority, and the compile or the make (the other null).</summary>
    public static (int Id, int Kind, ThreadPriority Priority, CompileRequest? Compile, MakeRequest? Make) ReadRequest(BinaryReader reader)
    {
        var id = reader.ReadInt32();
        var kind = reader.ReadInt32();
        var priority = (ThreadPriority)reader.ReadInt32();

        if (kind == CompileKind)
        {
            var profile = reader.ReadString();
            var path = reader.ReadString();
            var hlsl = reader.ReadString();

            return (id, kind, priority, new CompileRequest(path, hlsl, profile, DisableOptimizations: false), null);
        }

        var name = reader.ReadString();
        var programs = reader.ReadString();
        var folder = reader.ReadString();

        var model = reader.ReadString();
        var laneMin = reader.ReadInt32();
        var laneMax = reader.ReadInt32();
        var tier = reader.ReadString();
        var meshWave = reader.ReadInt32();

        var header = reader.ReadString();
        var adapter = new AdapterInfo(0, string.Empty, false, Optional(model), laneMin, laneMax, Optional(tier)) { MeshWaveLanes = OptionalLanes(meshWave) };

        return (id, kind, priority, null, new MakeRequest(name, programs, folder, adapter, header));
    }

    public static void WriteAnswer(BinaryWriter writer, int id, HostAnswer answer)
    {
        writer.Write(id);
        writer.Write((int)answer.Kind);
        writer.Write(answer.Text);

        writer.Write(answer.Profile);
        writer.Write(answer.Source);
        writer.Write(answer.Hlsl);
        writer.Flush();
    }

    public static (int Id, HostAnswer Answer) ReadAnswer(BinaryReader reader)
    {
        var id = reader.ReadInt32();
        var kind = (HostAnswerKind)reader.ReadInt32();
        var text = reader.ReadString();

        var profile = reader.ReadString();
        var source = reader.ReadString();
        var hlsl = reader.ReadString();

        return (id, new HostAnswer(kind, text, profile, source, hlsl));
    }

    private static string? Optional(string text)
    {
        if (text.Length == 0)
        {
            return null;
        }

        return text;
    }

    /// <summary>A measured lane count, or null for the 0 written when there is none.</summary>
    private static int? OptionalLanes(int lanes)
    {
        if (lanes == 0)
        {
            return null;
        }

        return lanes;
    }
}
