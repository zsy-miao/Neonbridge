// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Audio;
using FiveStar.Formats.Audio.Xma;
using FiveStar.Formats.Common;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>SHAPE XMA contexts decoded on the CPU, driven through the IAcpHal messages as the game's XMA voice drives them.</summary>
public sealed unsafe class XmaContextTests : IDisposable
{
    private const int FrameBlocks = 4;

    private readonly TestRuntime _test = TestRuntime.Create();

    [Fact]
    public void A_context_fills_its_ring_as_the_game_takes_blocks_and_gives_up_its_input_at_the_end()
    {
        var voice = new XmaVoiceDriver(_test, SilentXmaPackets.Build(2), 1);

        // Enabled: the ring fills (23 of the 24 blocks of six frames) and the output is no longer valid. The read
        // offset is at the seventh frame, the third of the second packet.
        voice.Enable();
        Assert.Equal(XmaVoiceDriver.Blocks, voice.Ready);
        Assert.False(voice.OutputValid);
        Assert.Equal(0u, voice.WriteOffset);
        Assert.Equal((2048u * 8) + 32 + (2 * SilentXmaPackets.FrameBits), voice.Context[2] & 0x3FFFFFF);

        // Taking 8 blocks makes room for the last two frames: input buffer 0 is used up and the current buffer is 1.
        var first = voice.Take(8);
        Assert.Equal(XmaVoiceDriver.Blocks, voice.Ready);
        Assert.False(voice.InputValid);
        Assert.Equal(1u, voice.Context[4] >> 31);

        var rest = voice.TakeAll(5);
        Assert.Equal(8 * FrameBlocks * XmaVoiceDriver.BlockSamples, first.Length + rest.Length);
        Assert.All(rest, sample => Assert.Equal(0, sample));
        Assert.True(voice.OutputValid);
    }

    [Fact]
    public void A_start_skip_and_a_loop_cut_and_repeat_subframes_as_the_context_fields_say()
    {
        var voice = new XmaVoiceDriver(_test, SilentXmaPackets.Build(2), 1);

        // Start skip 2 subframes; one loop from frame 1 (keeping subframes 0 and 1) back to frame 0 (skipping 1).
        voice.Context[0] |= 1u << 12;
        voice.Context[1] |= (2u << 24) | (1u << 12) | (1u << 17);
        voice.Context[3] = 32;
        voice.Context[4] = 32 + SilentXmaPackets.FrameBits;

        voice.Enable();
        var samples = voice.TakeAll(XmaVoiceDriver.Blocks);

        // Frame 0 (2 of 4), frame 1 (2), frame 0 again (3), frame 1 (4), frames 2 to 7 (24).
        Assert.Equal((2 + 2 + 3 + 4 + 24) * XmaVoiceDriver.BlockSamples, samples.Length);
        Assert.Equal(0u, (voice.Context[0] >> 12) & 0xFF);
        Assert.Equal(0u, (voice.Context[1] >> 24) & 7);
    }

    [DumpFact]
    public void An_xma2_sound_comes_out_of_its_context_as_the_whole_decoded_frames_in_16_bit_pcm()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "audio.rpf"));
        var track = new AwcTrackReader(source).Read(AwcReader.TryRead(source, 0x69DBC6000)!).Single(track => track.Id == 0x026F4E1Cu);
        var data = source.Read(track.DataOffset, (int)track.DataSize).ToArray();
        var reference = XmaTrackDecoder.Decode(data, 1, 32_000).Samples;

        var voice = new XmaVoiceDriver(_test, data.AsSpan(0, data.Length / 2048 * 2048), 1);
        voice.Enable();
        var pcm = voice.TakeAll(5);

        // Every frame's 512 samples: the sound's sample count. The track decoder's output is the same run with its
        // first frame and 64 samples dropped.
        Assert.Equal((int)track.Format.Frames, pcm.Length);

        for (int index = 0; index < reference.Length; index++)
        {
            float expected = Math.Clamp(MathF.Round(reference[index] * 32768), short.MinValue, short.MaxValue);
            Assert.InRange(pcm[XmaTrackDecoder.StartTrim + index] - expected, -1, 1);
        }
    }

    [DumpFact]
    public void A_voice_chained_over_two_contexts_plays_the_same_samples_as_one_context()
    {
        using var source = new MappedFileSource(Path.Combine(DumpFactAttribute.Folder!, "audio.rpf"));
        var track = new AwcTrackReader(source).Read(AwcReader.TryRead(source, 0x69DBC6000)!).Single(track => track.Id == 0x026F4E1Cu);
        var data = source.Read(track.DataOffset, (int)track.DataSize).ToArray();
        int split = data.Length / 2048 / 2 * 2048;

        var whole = new XmaVoiceDriver(_test, data, 1);
        whole.Enable();
        var continuous = whole.TakeAll(5);
        whole.Disable();

        // The game's chain: the first context plays the first half, hands its work buffer to the second, which then
        // continues the overlap. A second context started without the hand-over begins from silence instead.
        var first = whole.Chain(data.AsSpan(0, split), 1);
        var second = first.Chain(data.AsSpan(split), 1);
        first.Enable();
        var head = first.TakeAll(5);

        first.HandOver(second);
        second.Enable();
        var tail = second.TakeAll(5);

        var fresh = second.Chain(data.AsSpan(split), 1);
        fresh.Enable();
        var alone = fresh.TakeAll(5);

        Assert.Equal(continuous, head.Concat(tail).ToArray());
        Assert.NotEqual(tail, alone);
    }

    public void Dispose()
    {
        _test.Dispose();
    }
}
