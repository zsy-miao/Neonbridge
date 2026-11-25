// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The <c>--present-test</c> image, drawn with clears only: eight vertical color bars (white, yellow, cyan, green,
/// magenta, red, blue, black) over the top three quarters, and a white block that moves one step per present along a
/// dark bottom strip, so a still image and a running one look different.
/// </summary>
internal static unsafe class TestPattern
{
    /// <summary>ID3D12GraphicsCommandList::ClearRenderTargetView (d3d12.h).</summary>
    private const int ClearRenderTargetViewSlot = 48;

    private const int Bars = 8;
    private const int Channels = 4;

    /// <summary>The bars take this many quarters of the height; the strip the rest.</summary>
    private const int BarQuarters = 3;
    private const int Quarters = 4;

    /// <summary>The moving block is this fraction of the width, and moves this fraction per present.</summary>
    private const int BlockDivisor = 16;
    private const int StepDivisor = 128;

    private static readonly float[] Colors =
    [
        1f, 1f, 1f, 1f,
        1f, 1f, 0f, 1f,
        0f, 1f, 1f, 1f,
        0f, 1f, 0f, 1f,
        1f, 0f, 1f, 1f,
        1f, 0f, 0f, 1f,
        0f, 0f, 1f, 1f,
        0f, 0f, 0f, 1f,
    ];

    private static readonly float[] Strip = [0.1f, 0.1f, 0.1f, 1f];

    /// <summary>Records the clears into a render target view of a <paramref name="width"/> x <paramref name="height"/> buffer.</summary>
    [HotPath]
    public static void Record(nint list, nint view, uint width, uint height, long frame)
    {
        var clear = (delegate* unmanaged<nint, nint, float*, uint, PlaneRect*, void>)ComObject.Slot(list, ClearRenderTargetViewSlot);
        int barBottom = (int)(height * BarQuarters / Quarters);

        fixed (float* colors = Colors)
        {
            for (int bar = 0; bar < Bars; bar++)
            {
                var rect = new PlaneRect { Left = (int)(width * bar / Bars), Top = 0, Right = (int)(width * (bar + 1) / Bars), Bottom = barBottom };
                clear(list, view, colors + (bar * Channels), 1, &rect);
            }

            var bottom = new PlaneRect { Left = 0, Top = barBottom, Right = (int)width, Bottom = (int)height };

            fixed (float* strip = Strip)
            {
                clear(list, view, strip, 1, &bottom);
            }

            int block = (int)(width / BlockDivisor);
            int travel = (int)width - block;
            int left = 0;

            if (travel > 0)
            {
                left = (int)(frame * (width / StepDivisor) % travel);
            }

            var marker = new PlaneRect { Left = left, Top = barBottom, Right = left + block, Bottom = (int)height };
            clear(list, view, colors, 1, &marker);
        }
    }
}
