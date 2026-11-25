// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The one request and one answer of a pipeline host (<see cref="PipelineHost"/>) over its standard input and output.
/// The request: a tag, the adapter's LUID, the serialized root signature, the bytes to allocate instead of compiling (a
/// test of the memory limit; 0 compiles) and the stream (<see cref="DesktopStream.Write"/>). The answer: a tag and the
/// HRESULT of CreatePipelineState.
/// </summary>
internal static class PipelineHostProtocol
{
    /// <summary>The loader's command line switch that makes it a pipeline host.</summary>
    public const string Mode = "--pipeline-host";

    private const int RequestTag = 0x50484f31;
    private const int AnswerTag = 0x50484f32;

    public static void WriteRequest(BinaryWriter writer, PipelineHostRequest request, DesktopStream stream)
    {
        writer.Write(RequestTag);
        writer.Write(request.AdapterLuid);
        writer.Write(request.RootSignature.Length);
        writer.Write(request.RootSignature);
        writer.Write(request.AllocateBytes);

        stream.Write(writer);
        writer.Flush();
    }

    /// <summary>The request's fields; the stream follows and is read with <see cref="DesktopStream.Read"/>.</summary>
    public static PipelineHostRequest ReadRequest(BinaryReader reader)
    {
        if (reader.ReadInt32() != RequestTag)
        {
            throw new InvalidDataException("not a pipeline host request");
        }

        long luid = reader.ReadInt64();
        var root = reader.ReadBytes(reader.ReadInt32());

        return new PipelineHostRequest(luid, root, reader.ReadInt64());
    }

    public static void WriteAnswer(BinaryWriter writer, int hresult)
    {
        writer.Write(AnswerTag);
        writer.Write(hresult);
        writer.Flush();
    }

    /// <summary>The HRESULT the host answered.</summary>
    public static int ReadAnswer(BinaryReader reader)
    {
        if (reader.ReadInt32() != AnswerTag)
        {
            throw new InvalidDataException("not a pipeline host answer");
        }

        return reader.ReadInt32();
    }
}
