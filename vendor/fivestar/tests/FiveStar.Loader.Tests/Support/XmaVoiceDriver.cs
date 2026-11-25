// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Drives one SHAPE XMA context the way the game's XMA voice does: the start-up calls, the input in APU memory, the
/// context filled as 0x1400785d0 fills it (its 0x200-byte work buffer zeroed), message 5 to start, then per audio frame
/// the output copied from the read offset to the write offset (0x140078120) and acknowledged with message 0x12. A voice
/// that chains contexts gets the next one from <see cref="Chain"/>.
/// </summary>
internal sealed unsafe class XmaVoiceDriver
{
    public const int Blocks = 23;
    public const int BlockSamples = 128;

    private const int FirstIndex = 17;
    private const int ContextBytes = 64;
    private const int WorkBytes = 0x200;

    private readonly delegate* unmanaged<nint*, uint*, uint, uint, uint, int> _allocate;
    private readonly delegate* unmanaged<nint, uint, uint, uint, uint*, uint, int> _send;
    private readonly nint _hal;
    private readonly nint _contexts;
    private readonly uint _index;
    private readonly nint _output;
    private readonly nint _work;

    /// <summary>Sets up a mono context over <paramref name="data"/> (whole packets) with sample rate code <paramref name="rate"/>.</summary>
    public XmaVoiceDriver(TestRuntime test, ReadOnlySpan<byte> data, uint rate)
    {
        var createHeap = (delegate* unmanaged<uint, uint, int>)test.Shim("acphal.dll", "ApuCreateHeap");
        var allocateContexts = (delegate* unmanaged<byte*, int>)test.Shim("acphal.dll", "AcpHalAllocateShapeContexts");
        var create = (delegate* unmanaged<nint*, int>)test.Shim("acphal.dll", "AcpHalCreate");
        _allocate = (delegate* unmanaged<nint*, uint*, uint, uint, uint, int>)test.Shim("acphal.dll", "ApuAlloc");
        Assert.Equal(0, createHeap(0x1000000, 0x226E00));

        var block = stackalloc byte[0x40];
        new Span<byte>(block, 0x40).Clear();
        *(uint*)(block + 0x10) = 350;
        Assert.Equal(0, allocateContexts(block));
        _contexts = *(nint*)(block + 0x38);

        nint hal;
        Assert.Equal(0, create(&hal));
        _hal = hal;
        _send = (delegate* unmanaged<nint, uint, uint, uint, uint*, uint, int>)(*(nint**)hal)[2];

        _index = FirstIndex;
        (_output, _work) = Fill(data, rate);
    }

    private XmaVoiceDriver(XmaVoiceDriver previous, ReadOnlySpan<byte> data, uint rate)
    {
        _allocate = previous._allocate;
        _send = previous._send;
        (_hal, _contexts) = (previous._hal, previous._contexts);
        _index = previous._index + 1;
        (_output, _work) = Fill(data, rate);
    }

    public uint* Context => (uint*)(_contexts + (nint)(_index * ContextBytes));

    public uint WriteOffset => Context[0] >> 27;

    public uint ReadOffset => Context[9] & 0x1F;

    public bool OutputValid => (Context[1] >> 31) != 0;

    public bool InputValid => ((Context[0] >> 20) & 1) != 0;

    /// <summary>Blocks the game would see as ready: write minus read, or all of them when they meet and the output is not valid.</summary>
    public int Ready
    {
        get
        {
            int ready = (int)((WriteOffset - ReadOffset + Blocks) % Blocks);

            if (ready == 0 && !OutputValid)
            {
                return Blocks;
            }

            return ready;
        }
    }

    /// <summary>The next context, filled over <paramref name="data"/>, not yet enabled; it continues this one only after <see cref="HandOver"/>.</summary>
    public XmaVoiceDriver Chain(ReadOnlySpan<byte> data, uint rate)
    {
        return new XmaVoiceDriver(this, data, rate);
    }

    /// <summary>As the game hands a voice over to its next context (0x1400781b6): copy the work buffer, then disable this one.</summary>
    public void HandOver(XmaVoiceDriver next)
    {
        new ReadOnlySpan<byte>((void*)_work, WorkBytes).CopyTo(new Span<byte>((void*)next._work, WorkBytes));
        Disable();
    }

    public void Enable()
    {
        Send(5, 0xF6F6F6F6, 0);
    }

    public void Disable()
    {
        Send(7, _index + 1, 0);
    }

    /// <summary>Copies up to <paramref name="most"/> ready blocks from the read offset and acknowledges them.</summary>
    public short[] Take(int most)
    {
        int count = Math.Min(most, Ready);
        var samples = new short[count * BlockSamples];

        for (int block = 0; block < count; block++)
        {
            int at = (int)((ReadOffset + block) % Blocks) * BlockSamples;
            new ReadOnlySpan<short>((short*)_output + at, BlockSamples).CopyTo(samples.AsSpan(block * BlockSamples));
        }

        Send(0x12, 0xF1F1F1F1, (uint)count);

        return samples;
    }

    /// <summary>Takes blocks until the input is used up and nothing is ready.</summary>
    public short[] TakeAll(int most)
    {
        var all = new List<short>();

        while (InputValid || Ready > 0)
        {
            var taken = Take(most);
            Assert.NotEmpty(taken);
            all.AddRange(taken);
        }

        return [.. all];
    }

    /// <summary>
    /// The input, output and zeroed work buffer in APU memory, and the context as the game fills it: packets, buffer 0
    /// valid, 23 output blocks; 4 subframes per decode, rate, mono, output valid; read offset 32 (the first packet's
    /// header); both input pointers at the data; the output and work pointers.
    /// </summary>
    private (nint Output, nint Work) Fill(ReadOnlySpan<byte> data, uint rate)
    {
        var (input, inputApu) = Allocate((uint)data.Length, 0x800);
        var (output, outputApu) = Allocate(Blocks * 256, 0x100);
        var (work, workApu) = Allocate(WorkBytes, 0x100);
        data.CopyTo(new Span<byte>((void*)input, data.Length));
        new Span<byte>((void*)work, WorkBytes).Clear();

        new Span<uint>(Context, 16).Clear();
        Context[0] = (uint)(data.Length / 2048) | (1u << 20) | (Blocks << 22);
        Context[1] = (4u << 20) | (rate << 27) | (1u << 31);
        Context[2] = 32;
        (Context[5], Context[6], Context[7], Context[8]) = (inputApu, inputApu, outputApu, workApu);

        return (output, work);
    }

    private (nint Address, uint Apu) Allocate(uint bytes, uint alignment)
    {
        nint address;
        uint apu;
        Assert.Equal(0, _allocate(&address, &apu, bytes, alignment, 0));

        return (address, apu);
    }

    private void Send(uint type, uint cookie, uint blocks)
    {
        var payload = stackalloc uint[2];
        (payload[0], payload[1]) = (_index, blocks);

        Assert.Equal(0, _send(_hal, type, cookie, uint.MaxValue, payload, 0));
    }
}
