// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// What the audio hardware shims did, for the thread sampler's counter line: Opus commands and the samples they
/// produced, XMA context messages and the blocks written into their rings. Totals since start.
/// </summary>
internal sealed class AudioCounters
{
    private long _opusCreates;
    private long _opusDestroys;
    private long _opusDecodes;
    private long _opusFailures;
    private long _opusSamples;
    private long _opusResults;
    private long _xmaEnables;
    private long _xmaDisables;
    private long _xmaTakes;
    private long _xmaBlocks;
    private long _xmaUnmapped;
    private long _xmaEnded;

    public void OpusCreated() => Interlocked.Increment(ref _opusCreates);

    public void OpusDestroyed() => Interlocked.Increment(ref _opusDestroys);

    /// <summary>A decode command: <paramref name="samples"/> per channel written, 0 for a packet that did not decode.</summary>
    public void OpusDecoded(int samples, bool failed)
    {
        Interlocked.Increment(ref _opusDecodes);
        Interlocked.Add(ref _opusSamples, samples);

        if (failed)
        {
            Interlocked.Increment(ref _opusFailures);
        }
    }

    public void OpusResultTaken() => Interlocked.Increment(ref _opusResults);

    public void XmaEnabled() => Interlocked.Increment(ref _xmaEnables);

    public void XmaDisabled() => Interlocked.Increment(ref _xmaDisables);

    /// <summary>Message 0x12: the game took output from a ring.</summary>
    public void XmaTaken() => Interlocked.Increment(ref _xmaTakes);

    public void XmaBlocksWritten(int blocks) => Interlocked.Add(ref _xmaBlocks, blocks);

    /// <summary>A context whose output ring is not APU memory, so nothing can be written for it.</summary>
    public void XmaUnmapped() => Interlocked.Increment(ref _xmaUnmapped);

    /// <summary>A pass over a context that found its input used up.</summary>
    public void XmaInputEnded() => Interlocked.Increment(ref _xmaEnded);

    public string Summary =>
        $"Opus {Read(ref _opusCreates)} created, {Read(ref _opusDestroys)} destroyed, {Read(ref _opusDecodes)} packets " +
        $"({Read(ref _opusFailures)} failed, {Read(ref _opusSamples)} samples), {Read(ref _opusResults)} results taken; " +
        $"XMA {Read(ref _xmaEnables)} enables, {Read(ref _xmaDisables)} disables, {Read(ref _xmaTakes)} takes, " +
        $"{Read(ref _xmaBlocks)} blocks written, {Read(ref _xmaEnded)} passes out of input, {Read(ref _xmaUnmapped)} unmapped";

    private static long Read(ref long counter) => Interlocked.Read(ref counter);
}
