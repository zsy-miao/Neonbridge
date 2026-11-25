// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Concentus;
using Concentus.Enums;
using FiveStar.Formats.Audio;
using FiveStar.Formats.Common;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The console audio hardware on the CPU: the start-up calls succeed and Opus decodes through the xapu.dll queue.</summary>
public sealed unsafe class AudioHardwareTests : IDisposable
{
    private const int Rate = 48_000;
    private const int Frame = 960;

    private readonly TestRuntime _test = TestRuntime.Create();

    [Fact]
    public void Audio_start_up_gets_its_xma_contexts_and_an_acphal_object_that_accepts_its_calls()
    {
        var createHeap = (delegate* unmanaged<uint, uint, int>)_test.Shim("acphal.dll", "ApuCreateHeap");
        var allocate = (delegate* unmanaged<byte*, int>)_test.Shim("acphal.dll", "AcpHalAllocateShapeContexts");
        var isValid = (delegate* unmanaged<nint, uint, int>)_test.Shim("acphal.dll", "ApuIsVirtualAddressValid");
        var create = (delegate* unmanaged<nint*, int>)_test.Shim("acphal.dll", "AcpHalCreate");
        Assert.Equal(0, createHeap(0x400000, 0x226E00));

        // SHAPE_CONTEXTS as the game fills it: only the XMA count (350), the array pointer comes back at +0x38.
        var block = stackalloc byte[0x40];
        new Span<byte>(block, 0x40).Clear();
        *(uint*)(block + 0x10) = 350;
        Assert.Equal(0, allocate(block));
        nint contexts = *(nint*)(block + 0x38);
        Assert.Equal(1, isValid(contexts + (350 * 64) - 1, 0));

        nint hal;
        Assert.Equal(0, create(&hal));
        var vtable = *(nint**)hal;
        uint payload = 0xC0;
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, uint, int>)vtable[0])(hal, 0x1000, 0x1000));
        Assert.Equal(0, ((delegate* unmanaged<nint, uint, uint, uint, uint*, uint, int>)vtable[2])(hal, 2, 0xF0F0F0F0, uint.MaxValue, &payload, 0));

        *(uint*)block = 4;
        Assert.Equal(unchecked((int)0x80004001), allocate(block));
    }

    [Fact]
    public void A_stereo_opus_stream_decodes_through_the_xapu_queue_as_the_game_drives_it()
    {
        var encoder = OpusCodecFactory.CreateEncoder(Rate, 2, OpusApplication.OPUS_APPLICATION_AUDIO);
        var driver = new XApuDriver(_test, 7, 2);
        var pcm = new float[Frame * 2];
        var packet = new byte[1275];
        var levels = new List<double>();

        for (int frame = 0; frame < 20; frame++)
        {
            for (int sample = 0; sample < Frame; sample++)
            {
                float value = 0.5f * MathF.Sin(2 * MathF.PI * 440 * ((frame * Frame) + sample) / Rate);
                pcm[2 * sample] = value;
                pcm[(2 * sample) + 1] = value;
            }

            int length = encoder.Encode(pcm, Frame, packet, packet.Length);
            float[] decoded = driver.Decode(packet.AsSpan(0, length));

            Assert.Equal(Frame * 2, decoded.Length);
            levels.Add(Math.Sqrt(decoded.Average(value => (double)value * value)));
        }

        // A 0.5 sine has an RMS level of 0.354 once the codec delay has passed.
        Assert.All(levels.Skip(2), level => Assert.InRange(level, 0.3, 0.4));
    }

    [Fact]
    public void An_opus_decode_drops_the_samples_its_first_sample_field_names_and_echoes_the_command()
    {
        var encoder = OpusCodecFactory.CreateEncoder(Rate, 1, OpusApplication.OPUS_APPLICATION_AUDIO);
        var whole = new XApuDriver(_test, 3, 1);
        var trimmed = new XApuDriver(_test, 4, 1);
        var pcm = new float[Frame];
        var packet = new byte[1275];

        for (int frame = 0; frame < 3; frame++)
        {
            for (int sample = 0; sample < Frame; sample++)
            {
                pcm[sample] = 0.5f * MathF.Sin(2 * MathF.PI * 440 * ((frame * Frame) + sample) / Rate);
            }

            int length = encoder.Encode(pcm, Frame, packet, packet.Length);
            float[] expected = whole.Decode(packet.AsSpan(0, length));
            float[] decoded = trimmed.Decode(packet.AsSpan(0, length), 312);

            Assert.Equal(expected.AsSpan(312).ToArray(), decoded);
        }
    }

    [Fact]
    public void An_xdsp_convolution_stream_completes_each_command_where_the_game_polls_and_returns_silence()
    {
        var connect = (delegate* unmanaged<nint, uint, uint, uint, uint, nint*, int>)_test.Shim("chad.dll", "XDspConnectWithMaximumStreamLimit");
        var activate = (delegate* unmanaged<nint, byte*, nint*, nint*, int>)_test.Shim("chad.dll", "XDspActivate");
        var submit = (delegate* unmanaged<nint, byte*, uint*, int>)_test.Shim("chad.dll", "XDspSubmitCommand");
        var deactivate = (delegate* unmanaged<nint, int>)_test.Shim("chad.dll", "XDspDeactivate");

        // The game's 0x60000-byte pool, zeroed, and the start-up connect with its arguments (0x18 streams, 0x80, 1).
        var pool = new float[0x60000 / sizeof(float)];
        nint handle;

        fixed (float* memory = pool)
        {
            Assert.Equal(0, connect((nint)memory, 0x60000, 0x18, 0x80, 1, &handle));

            // A stereo convolution config as the game fills it: kind 1, 1024 frames, 2, response length and address.
            var config = stackalloc uint[8];
            new Span<uint>(config, 8).Clear();
            (config[0], config[2], config[3], config[4]) = (1, 1024, 2, 256);
            nint status;
            nint stream;
            Assert.Equal(0, activate(handle, (byte*)config, &status, &stream));

            // Each command: 1.0f twice, input block, output block, 0; the id it returns is the completed count.
            pool.AsSpan(0, 2048).Fill(0.5f);
            var command = stackalloc long[4];
            (command[0], command[1], command[2], command[3]) = (0x3F8000003F800000, (nint)memory, (nint)(memory + 2048), 0);

            for (uint expected = 1; expected <= 3; expected++)
            {
                uint id;
                Assert.Equal(0, submit(stream, (byte*)command, &id));
                Assert.Equal(expected, id);
                Assert.Equal(id, *(uint*)(status + 0x04));
                Assert.Equal(0u, *(uint*)(status + 0x0C));
            }

            Assert.All(pool.AsSpan(2048, 2048).ToArray(), sample => Assert.Equal(0f, sample));
            Assert.Equal(0, deactivate(stream));
            Assert.NotEqual(0, submit(stream, (byte*)command, null));
        }
    }

    [DumpFact]
    public void A_bank_stream_decodes_through_the_xapu_queue_to_the_audio_of_its_exported_file()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "audio.rpf"));
        var track = new AwcTrackReader(source).Read(AwcReader.TryRead(source, 0x9FAA000)!).Single();
        using var file = new MemoryStream();
        OggOpusWriter.Write(file, source, track, 1);

        // The exported file's packets (after OpusHead and OpusTags) on a decoder of their own.
        var exported = OggPackets.Read(file.ToArray()).Skip(2).ToList();
        var reference = OpusCodecFactory.CreateDecoder(Rate, 1);
        var driver = new XApuDriver(_test, 0x21, 1);
        var expected = new float[Frame];

        Assert.Equal(track.Packets.Count, exported.Count);

        for (int index = 0; index < exported.Count; index++)
        {
            var packet = track.Packets[index];
            float[] decoded = driver.Decode(source.Read(packet.Offset, packet.Length));
            Assert.Equal(Frame, reference.Decode(exported[index], expected, Frame, false));

            Assert.Equal(expected, decoded);
        }
    }

    public void Dispose()
    {
        _test.Dispose();
    }
}
