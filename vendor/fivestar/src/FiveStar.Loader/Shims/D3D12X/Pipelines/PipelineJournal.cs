// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The pipelines the game used, in the order it first used them, across boots: <c>pipelines-used.bin</c> in the
/// pipeline cache folder (<see cref="PipelineJournalFile"/>). Each record is a game description (every creation slot,
/// derives included) with its shaders as code-block prefixes, so it names the pipeline whatever the translation: the
/// next boot builds these at creation, the warm-up builds them first.
/// </summary>
internal sealed class PipelineJournal : IDisposable
{
    /// <summary>The journal's file name in the pipeline cache folder.</summary>
    public const string FileName = PipelineJournalFile.FileName;

    private const byte GraphicsKind = PipelineJournalFile.GraphicsKind;
    private const byte ComputeKind = PipelineJournalFile.ComputeKind;

    private readonly Lock _gate = new();
    private readonly HashSet<string> _known = new(StringComparer.Ordinal);
    private readonly FileStream _file;
    private readonly BinaryWriter _writer;

    private PipelineJournal(FileStream file, IEnumerable<byte[]> records)
    {
        _file = file;
        _writer = new BinaryWriter(file);

        foreach (var record in records)
        {
            _known.Add(Hash(record));
        }
    }

    /// <summary>Descriptions recorded so far, this run's included.</summary>
    public int Count
    {
        get
        {
            lock (_gate)
            {
                return _known.Count;
            }
        }
    }

    /// <summary>The journal in <paramref name="folder"/> for appending; a damaged one starts over.</summary>
    public static PipelineJournal Open(string folder)
    {
        var path = Path.Combine(folder, FileName);
        var records = PipelineJournalFile.Records(path);
        var file = new FileStream(path, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.Read);

        if (records is null)
        {
            file.SetLength(0);
            var header = new BinaryWriter(file);
            header.Write(PipelineJournalFile.Magic);
            header.Write(PipelineJournalFile.Version);
            header.Flush();
            records = [];
        }
        else
        {
            // A record cut off by a crash is dropped, so the next one starts where it started.
            file.SetLength(PipelineJournalFile.HeaderSize + records.Sum(record => (long)sizeof(int) + record.Length));
        }

        file.Seek(0, SeekOrigin.End);

        return new PipelineJournal(file, records);
    }

    /// <summary>The descriptions of the journal in <paramref name="folder"/> in first-use order; none without one.</summary>
    public static List<WarmTarget> Read(string folder)
    {
        var targets = new List<WarmTarget>();

        foreach (var record in PipelineJournalFile.Records(Path.Combine(folder, FileName)) ?? [])
        {
            var target = Decode(record, targets.Count);

            if (target is not null)
            {
                targets.Add(target);
            }
        }

        return targets;
    }

    /// <summary>Appends a graphics description the journal does not have yet.</summary>
    public void Record(GraphicsPipelineDescription description)
    {
        Append(Encode(description));
    }

    /// <summary>Appends a compute description the journal does not have yet.</summary>
    public void Record(ComputePipelineDescription description)
    {
        Append(Encode(description));
    }

    /// <summary>Whether an earlier boot (or this one) asked for the description.</summary>
    public bool Contains(GraphicsPipelineDescription description)
    {
        return Known(Encode(description));
    }

    /// <summary>Whether an earlier boot (or this one) asked for the description.</summary>
    public bool Contains(ComputePipelineDescription description)
    {
        return Known(Encode(description));
    }

    public void Dispose()
    {
        lock (_gate)
        {
            _writer.Dispose();
            _file.Dispose();
        }
    }

    private static byte[]? Encode(GraphicsPipelineDescription description)
    {
        return Encode(GraphicsKind, writer => Write(writer, description));
    }

    private static byte[]? Encode(ComputePipelineDescription description)
    {
        return Encode(ComputeKind, writer => WriteShader(writer, description.Shader));
    }

    /// <summary>The record of a description; null when a shader has no readable code block (the creation fails anyway).</summary>
    private static byte[]? Encode(byte kind, Action<BinaryWriter> write)
    {
        try
        {
            using var buffer = new MemoryStream();
            using var writer = new BinaryWriter(buffer);
            writer.Write(kind);
            write(writer);
            writer.Flush();

            return buffer.ToArray();
        }
        catch (InvalidDataException)
        {
            return null;
        }
    }

    private bool Known(byte[]? record)
    {
        if (record is null)
        {
            return false;
        }

        lock (_gate)
        {
            return _known.Contains(Hash(record));
        }
    }

    private void Append(byte[]? record)
    {
        if (record is null)
        {
            return;
        }

        lock (_gate)
        {
            if (!_known.Add(Hash(record)))
            {
                return;
            }

            _writer.Write(record.Length);
            _writer.Write(record);

            // Flushed per record: a boot that crashes keeps what it asked for.
            _writer.Flush();
        }
    }

    private static void Write(BinaryWriter writer, GraphicsPipelineDescription description)
    {
        writer.Write(description.Shaders.Count);

        foreach (var shader in description.Shaders)
        {
            WriteShader(writer, shader);
        }

        WriteBytes(writer, description.Blend);
        writer.Write(description.SampleMask);
        WriteBytes(writer, description.Rasterizer);
        WriteBytes(writer, description.DepthStencil);
        writer.Write(description.InputLayout.Count);

        foreach (var element in description.InputLayout)
        {
            writer.Write(element.Semantic);
            writer.Write(element.SemanticIndex);
            writer.Write(element.Format);

            writer.Write(element.InputSlot);
            writer.Write(element.Offset);
            writer.Write(element.Classification);
            writer.Write(element.StepRate);
        }

        writer.Write(description.StripCut);
        writer.Write(description.TopologyType);
        writer.Write(description.RenderTargetFormats.Count);

        foreach (var format in description.RenderTargetFormats)
        {
            writer.Write(format);
        }

        writer.Write(description.DepthStencilFormat);
        writer.Write(description.SampleCount);
        writer.Write(description.SampleQuality);
    }

    /// <summary>A shader as the code-block prefix the translated programs are found by.</summary>
    private static void WriteShader(BinaryWriter writer, ShaderSource shader)
    {
        WriteBytes(writer, ProgramKeys.Prefix(shader));
    }

    private static void WriteBytes(BinaryWriter writer, byte[] bytes)
    {
        writer.Write(bytes.Length);
        writer.Write(bytes);
    }

    private static WarmTarget? Decode(byte[] record, int index)
    {
        try
        {
            using var reader = new BinaryReader(new MemoryStream(record));

            if (reader.ReadByte() == ComputeKind)
            {
                return new WarmTarget(index, null, new ComputePipelineDescription(0, ReadShader(reader)));
            }

            return new WarmTarget(index, ReadGraphics(reader), null);
        }
        catch (EndOfStreamException)
        {
            return null;
        }
    }

    private static GraphicsPipelineDescription ReadGraphics(BinaryReader reader)
    {
        var shaders = Enumerable.Range(0, reader.ReadInt32()).Select(_ => ReadShader(reader)).ToList();
        var blend = ReadBytes(reader);
        var sampleMask = reader.ReadUInt32();
        var rasterizer = ReadBytes(reader);
        var depthStencil = ReadBytes(reader);

        var elements = Enumerable.Range(0, reader.ReadInt32())
            .Select(_ => new VertexElement(reader.ReadString(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32()))
            .ToList();
        var stripCut = reader.ReadInt32();
        var topology = reader.ReadInt32();
        var formats = Enumerable.Range(0, reader.ReadInt32()).Select(_ => reader.ReadInt32()).ToList();

        return new GraphicsPipelineDescription(0, shaders, blend, sampleMask, rasterizer, depthStencil, elements, stripCut, topology, formats, reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32());
    }

    private static ShaderSource ReadShader(BinaryReader reader)
    {
        return new ShaderSource(ShaderSourceKind.CacheCode, ReadBytes(reader));
    }

    private static byte[] ReadBytes(BinaryReader reader)
    {
        var length = reader.ReadInt32();
        var bytes = reader.ReadBytes(length);

        if (bytes.Length != length)
        {
            throw new EndOfStreamException();
        }

        return bytes;
    }

    private static string Hash(byte[] record)
    {
        return Convert.ToHexString(SHA256.HashData(record));
    }
}
