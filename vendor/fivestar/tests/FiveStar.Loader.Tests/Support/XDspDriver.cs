// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Drives the chad.dll shims the way the game's convolution effect does (0x14003b770, 0x144e5acc0, 0x1443b2a40): connect,
/// activate a convolution with a response in memory, then per block the channels one after the other into the input page,
/// a command of two 1.0 gains and the page pair, and the output page once the status record says the command is done.
/// </summary>
internal sealed unsafe class XDspDriver : IDisposable
{
    private const int Page = 0x1000;

    private readonly delegate* unmanaged<nint, byte*, uint*, int> _submit;
    private readonly float* _pages;
    private readonly float* _response;
    private readonly int _channels;
    private readonly int _frames;
    private readonly byte* _status;
    private readonly nint _stream;

    /// <summary>A convolution stream of <paramref name="channels"/> channels, <paramref name="frames"/> frames each per command.</summary>
    public XDspDriver(TestRuntime test, ReadOnlySpan<float> response, int channels, int frames)
    {
        var connect = (delegate* unmanaged<nint, uint, uint, uint, uint, nint*, int>)test.Shim("chad.dll", "XDspConnectWithMaximumStreamLimit");
        var activate = (delegate* unmanaged<nint, byte*, nint*, nint*, int>)test.Shim("chad.dll", "XDspActivate");
        _submit = (delegate* unmanaged<nint, byte*, uint*, int>)test.Shim("chad.dll", "XDspSubmitCommand");
        (_channels, _frames) = (channels, frames);

        _pages = (float*)NativeMemory.AllocZeroed(2 * Page);
        _response = (float*)NativeMemory.Alloc((nuint)response.Length, sizeof(float));
        response.CopyTo(new Span<float>(_response, response.Length));

        nint handle;
        Assert.Equal(0, connect(0, 0x60000, 0x18, 0x80, 1, &handle));

        // The config as 0x14003b770 fills it: kind 1, 1024 frames, the response's channels, length and address.
        var config = stackalloc byte[0x20];
        new Span<byte>(config, 0x20).Clear();
        (*(uint*)config, *(uint*)(config + 0x08), *(uint*)(config + 0x0C)) = (1, 1024, (uint)channels);
        (*(uint*)(config + 0x10), *(long*)(config + 0x14)) = ((uint)response.Length, (long)_response);

        nint status;
        nint stream;
        Assert.Equal(0, activate(handle, config, &status, &stream));
        _status = (byte*)status;
        _stream = stream;
    }

    /// <summary>Runs one command over <paramref name="input"/> (per channel, one block) and returns the output per channel.</summary>
    public float[][] Process(float[][] input)
    {
        for (int channel = 0; channel < _channels; channel++)
        {
            input[channel].CopyTo(new Span<float>(_pages + (channel * _frames), _frames));
        }

        var command = stackalloc byte[0x20];
        new Span<byte>(command, 0x20).Clear();
        (*(float*)command, *(float*)(command + 4)) = (1f, 1f);
        (*(long*)(command + 0x08), *(long*)(command + 0x10)) = ((long)_pages, (long)_pages + Page);

        uint id;
        Assert.Equal(0, _submit(_stream, command, &id));
        Assert.Equal(id, *(uint*)(_status + 4));
        Assert.Equal(0u, *(uint*)(_status + 0x0C));

        var output = new float[_channels][];

        for (int channel = 0; channel < _channels; channel++)
        {
            output[channel] = new ReadOnlySpan<float>(_pages + (Page / sizeof(float)) + (channel * _frames), _frames).ToArray();
        }

        return output;
    }

    public void Dispose()
    {
        NativeMemory.Free(_pages);
        NativeMemory.Free(_response);
    }
}
