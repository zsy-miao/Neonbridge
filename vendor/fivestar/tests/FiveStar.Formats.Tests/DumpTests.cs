// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Formats.Audio;
using FiveStar.Formats.Audio.Xma;
using FiveStar.Formats.Common;
using FiveStar.Formats.Compression;
using FiveStar.Formats.Pso;
using FiveStar.Formats.Rpf;
using FiveStar.Formats.Tests.Support;
using FiveStar.Formats.Ttpf;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>The readers on the user's own dump (FIVESTAR_DUMP); the counts are those of the shipped package.</summary>
public sealed class DumpTests
{
    [DumpFact]
    public void Script_rpf_carves_into_1013_pso_files_that_all_walk()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "script.rpf"));
        var carver = new ZlibEntryCarver(source, new ZlibStreamProbe(source, new ZlibInflater()));

        var blobs = carver.Carve(0).ToList();
        var roots = new HashSet<uint>();

        foreach (var blob in blobs.Where(blob => blob.Kind == BlobKind.Psin))
        {
            var document = PsoReader.Load(blob.Data);
            new PsoWalker(document).WalkRoot(PsoWalker.UnlimitedBudget);
            roots.Add(document.BlockAt(document.Root!.Value).Type);
        }

        Assert.Equal(1013, blobs.Count);
        Assert.Equal(1012, blobs.Count(blob => blob.Kind == BlobKind.Psin));
        Assert.Equal([0xDE396FE2u], roots);
    }

    [DumpFact]
    public void An_audio_bank_stream_holds_opus_packets_that_cover_its_sample_count()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "audio.rpf"));

        var container = AwcReader.TryRead(source, 0x9FAA000)!;
        var track = new AwcTrackReader(source).Read(container).Single();

        Assert.Equal(0x30B0, container.Size);
        Assert.Equal(0x053B07D5u, track.Id);
        Assert.Null(track.Problem);

        Assert.Equal(AwcCodec.Opus, track.Codec);
        Assert.Equal(new AwcFormat(135_555, 48_000, 109, 0x0B, 1), track.Format);
        Assert.Equal(144, track.Packets.Count);
        Assert.Equal(144 * 960, track.PacketSamples);
    }

    [DumpFact]
    public void A_multichannel_bank_splits_into_two_stereo_tracks_across_its_blocks()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "audio.rpf"));

        var container = AwcReader.TryRead(source, 0x2CCBF0000)!;
        var tracks = new AwcTrackReader(source).Read(container).ToList();

        Assert.Equal([0x1DE4E830u, 0x0AA7DD46u], tracks.Select(track => track.Id));
        Assert.All(tracks, track => Assert.Null(track.Problem));
        Assert.All(tracks, track => Assert.Equal(2, track.Format.Channels));
        Assert.All(tracks, track => Assert.Equal(16_778_489u, track.Format.Frames));
        Assert.All(tracks, track => Assert.InRange(track.PacketSamples - track.Format.Frames, OggOpusWriter.PreSkip, 2880));
    }

    [DumpFact]
    public void An_xma2_sound_decodes_every_frame_cleanly_with_no_step_at_the_frame_joins()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "audio.rpf"));
        var track = XmaTracks(source, 0x69DBC6000).Single(track => track.Id == 0x026F4E1Cu);

        var sound = XmaTrackDecoder.Decode(source.Read(track.DataOffset, (int)track.DataSize), track.Format.Channels, (int)track.Format.SampleRate);
        var samples = sound.Samples;
        var steps = Enumerable.Range(1, samples.Length - 1).Select(index => (Index: index, Step: Math.Abs(samples[index] - samples[index - 1]))).ToList();
        var atJoins = steps.Where(step => (step.Index + XmaTrackDecoder.StartTrim) % 512 == 0).Average(step => step.Step);

        Assert.Equal(new AwcFormat(58_880, 32_000, track.Format.Headroom, (byte)AwcCodec.Xma2, 1), track.Format);
        Assert.Equal(58_880 / 512, sound.Frames);
        Assert.Equal(0, sound.BadFrames);
        Assert.Equal(58_112, samples.Length);

        Assert.InRange(Math.Sqrt(samples.Average(sample => (double)sample * sample)), 0.01, 1.0);
        Assert.InRange(atJoins / steps.Average(step => step.Step), 0.8, 1.25);
    }

    [FfmpegDumpFact]
    public void The_xma2_sounds_of_a_bank_decode_like_ffmpeg()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "audio.rpf"));
        var tracks = XmaTracks(source, 0x69DBC6000);

        foreach (var track in tracks)
        {
            var data = source.Read(track.DataOffset, (int)track.DataSize);
            var sound = XmaTrackDecoder.Decode(data, track.Format.Channels, (int)track.Format.SampleRate);
            var reference = DecodeWithFfmpeg(track.Format, data.ToArray());

            Assert.Equal(reference.Length, sound.Samples.Length);
            Assert.InRange(reference.Zip(sound.Samples, (expected, actual) => Math.Abs(expected - actual)).Max(), 0, 1e-5);
        }

        Assert.Equal(11, tracks.Count);
    }

    [DumpFact]
    public void The_graphics_page_file_extracts_906_textures()
    {
        using var source = new MappedFileSource(GraphicsPageFile());
        var file = new TexturePageFile(source, new ZlibInflater());
        var extractor = new TextureExtractor(file);

        var statuses = file.Descriptors.Select(descriptor => extractor.ReadMip0(descriptor).Status.Name).ToList();

        Assert.Equal(912, statuses.Count);
        Assert.Equal(906, statuses.Count(status => status == "ok"));
        Assert.Equal(6, statuses.Count(status => status == "skip_unsupported_codec"));
    }

    [DumpFact]
    public void Every_texture_layout_fits_its_span()
    {
        var statuses = new Dictionary<string, int>();

        foreach (var path in Directory.GetFiles(DumpFactAttribute.Folder!, "*.ttpf", SearchOption.AllDirectories))
        {
            using var source = new MappedFileSource(path);
            var file = new TexturePageFile(source, new ZlibInflater());
            var extractor = new TextureExtractor(file);

            foreach (var descriptor in file.Descriptors)
            {
                var name = extractor.Locate(descriptor).Status.Name;
                statuses[name] = statuses.GetValueOrDefault(name) + 1;
            }
        }

        Assert.Equal(new Dictionary<string, int> { ["ok"] = 357_115 }, statuses);
    }

    /// <summary>
    /// Texture #901 is a 256x32x8 R16_FLOAT volume. Read as 3D bricks its depth slices are near copies of each other
    /// (a mean step of 6 in the raw 16-bit values); read as 2D slices back to back the step is about 130.
    /// </summary>
    [DumpFact]
    public void A_volume_texture_reads_as_3d_bricks()
    {
        using var source = new MappedFileSource(GraphicsPageFile());
        var file = new TexturePageFile(source, new ZlibInflater());

        var (status, data, layout) = new TextureExtractor(file).ReadMip0(file.Descriptors[901]);
        var values = System.Runtime.InteropServices.MemoryMarshal.Cast<byte, ushort>(data).ToArray();
        var slice = layout!.BlockWidth * layout.BlockHeight;
        var step = Enumerable.Range(0, values.Length - slice).Average(index => Math.Abs(values[index + slice] - values[index]));

        Assert.True(status.IsOk);
        Assert.Equal((TextureKind.Volume, 256, 32, 8), (layout.Kind, layout.BlockWidth, layout.BlockHeight, layout.Depth));
        Assert.InRange(step, 0, 32);
    }

    private static string GraphicsPageFile() => Path.Combine(DumpFactAttribute.Folder!, "data", "ttdb", "graphics", "ttdb_graphics.ttpf");

    private static List<AwcTrack> XmaTracks(MappedFileSource source, long container) =>
        new AwcTrackReader(source).Read(AwcReader.TryRead(source, container)!).Where(track => track.Codec == AwcCodec.Xma2).ToList();

    /// <summary>The track wrapped in an XMA2 WAVE file and decoded by FFmpeg to interleaved 32-bit floats.</summary>
    private static float[] DecodeWithFfmpeg(AwcFormat format, byte[] data)
    {
        var wave = Path.GetTempFileName();

        try
        {
            using (var file = File.Create(wave))
            {
                WaveWriter.Write(file, format, data);
            }

            var start = new ProcessStartInfo(FfmpegDumpFactAttribute.Program!, ["-hide_banner", "-loglevel", "error", "-i", wave, "-f", "f32le", "pipe:1"])
            {
                RedirectStandardOutput = true,
            };
            using var ffmpeg = Process.Start(start)!;
            using var pcm = new MemoryStream();
            ffmpeg.StandardOutput.BaseStream.CopyTo(pcm);
            ffmpeg.WaitForExit();

            return System.Runtime.InteropServices.MemoryMarshal.Cast<byte, float>(pcm.ToArray()).ToArray();
        }
        finally
        {
            File.Delete(wave);
        }
    }
}
