// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Pipelines;
using Vortice.Direct3D12;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The pipeline host: a stream survives the trip to the host, a host past its memory limit is ended and counted as
/// killed, and the outcomes and limits follow the rules.
/// </summary>
public sealed class PipelineHostTests
{
    private const long Gigabyte = 1L << 30;

    [Fact]
    public void A_stream_written_for_the_host_reads_back_the_same_with_its_root_signature_filled_in()
    {
        using var stream = new DesktopStream();
        stream.AddPointer(PipelineStateSubObjectType.RootSignature, 0x1234);
        stream.AddShader(PipelineStateSubObjectType.VertexShader, [1, 2, 3, 4, 5]);
        stream.AddInputLayout([new VertexElement("POSITION", 0, 6, 0, 0, 0, 0), new VertexElement("TEXCOORD", 1, 16, 1, 12, 1, 1)]);
        stream.AddWords(PipelineStateSubObjectType.PrimitiveTopology, [3]);

        var first = Written(stream);
        using var reader = new BinaryReader(new MemoryStream(first));
        using var read = DesktopStream.Read(reader, 0x1234);

        Assert.Equal(first, Written(read));
        Assert.Equal(5, read.ShaderBytes);

        using var moved = DesktopStream.Read(new BinaryReader(new MemoryStream(first)), 0x5678);
        Assert.NotEqual(first, Written(moved));

        // The host reads from a pipe, which has no length (boot w28: every host ended on asking it).
        using var piped = DesktopStream.Read(new BinaryReader(new Unseekable(first)), 0x1234);
        Assert.Equal(first, Written(piped));

        // A shader slot past the stream's bytes, and a length past the data, are refused before anything is written.
        var outside = (byte[])first.Clone();
        BitConverter.TryWriteBytes(outside.AsSpan(4 + BitConverter.ToInt32(first) + 8), 4096);
        Assert.Throws<InvalidDataException>(() => DesktopStream.Read(new BinaryReader(new MemoryStream(outside)), 0x1234));
        var longer = (byte[])first.Clone();
        BitConverter.TryWriteBytes(longer, int.MaxValue);
        Assert.Throws<InvalidDataException>(() => DesktopStream.Read(new BinaryReader(new MemoryStream(longer)), 0x1234));
    }

    [Fact]
    public void A_host_past_its_memory_limit_is_ended_and_counted_as_killed()
    {
        var host = new PipelineHost(Path.Combine(AppContext.BaseDirectory, "fivestar-loader.exe"));
        using var stream = new DesktopStream();
        var request = new PipelineHostRequest(0, [], 2 * Gigabyte);

        var result = host.Build(request, stream, Gigabyte / 2, TimeSpan.FromSeconds(60));

        Assert.Equal(PipelineHostOutcome.KilledMemory, result.Outcome);
        Assert.InRange(result.PeakBytes, Gigabyte * 3 / 8, Gigabyte / 2);
        Assert.StartsWith("pipeline host: 0 built, 1 past a limit", host.Summary, StringComparison.Ordinal);

        // Within its limit the same host answers.
        Assert.Equal(PipelineHostOutcome.Built, host.Build(request with { AllocateBytes = Gigabyte / 8 }, stream, Gigabyte / 2, TimeSpan.FromSeconds(60)).Outcome);
    }

    [Fact]
    public void Outcomes_and_limits_follow_the_answer_the_peak_and_the_vendor()
    {
        Assert.Equal(PipelineHostOutcome.Built, PipelineHost.Classify(0, 7 * Gigabyte, 24 * Gigabyte, timedOut: false));
        Assert.Equal(PipelineHostOutcome.KilledTime, PipelineHost.Classify(null, Gigabyte, 24 * Gigabyte, timedOut: true));
        Assert.Equal(PipelineHostOutcome.KilledMemory, PipelineHost.Classify(null, 23 * Gigabyte, 24 * Gigabyte, timedOut: false));
        Assert.Equal(PipelineHostOutcome.KilledMemory, PipelineHost.Classify(unchecked((int)0x8007000E), 24 * Gigabyte, 24 * Gigabyte, timedOut: false));
        Assert.Equal(PipelineHostOutcome.Failed, PipelineHost.Classify(unchecked((int)0x80070057), Gigabyte, 24 * Gigabyte, timedOut: false));
        Assert.Equal(PipelineHostOutcome.Crashed, PipelineHost.Classify(null, Gigabyte, 24 * Gigabyte, timedOut: false));

        // Free memory less the 4 GB floor, within 4 GB and the vendor's cap.
        Assert.Equal(24 * Gigabyte, PipelineHost.MemoryLimit(60UL * Gigabyte, 0x10DE));
        Assert.Equal(48 * Gigabyte, PipelineHost.MemoryLimit(60UL * Gigabyte, CompileBudget.AmdVendorId));
        Assert.Equal(10 * Gigabyte, PipelineHost.MemoryLimit(14UL * Gigabyte, 0x10DE));
        Assert.Equal(4 * Gigabyte, PipelineHost.MemoryLimit(5UL * Gigabyte, 0x10DE));
    }

    /// <summary>A stream that, like a pipe, cannot seek or tell its length.</summary>
    private sealed class Unseekable(byte[] bytes) : MemoryStream(bytes)
    {
        public override bool CanSeek => false;

        public override long Length => throw new NotSupportedException();

        public override long Position
        {
            get => throw new NotSupportedException();
            set => throw new NotSupportedException();
        }
    }

    private static byte[] Written(DesktopStream stream)
    {
        using var memory = new MemoryStream();

        using (var writer = new BinaryWriter(memory, System.Text.Encoding.UTF8, leaveOpen: true))
        {
            stream.Write(writer);
        }

        return memory.ToArray();
    }
}
