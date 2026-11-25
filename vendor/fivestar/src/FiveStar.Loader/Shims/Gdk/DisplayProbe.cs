// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>Reads the first display output of the first adapter that has one through DXGI: the primary display.</summary>
internal static unsafe class DisplayProbe
{
    /// <summary>The primary display, or null when DXGI or the display cannot answer.</summary>
    public static HdrDisplay? Primary()
    {
        Guid id = Dxgi.Factory1Id;
        nint factory = 0;

        if (Dxgi.CreateDXGIFactory1(&id, &factory) < 0)
        {
            return null;
        }

        try
        {
            return FirstDisplay(factory);
        }
        finally
        {
            ComObject.Release(factory);
        }
    }

    private static HdrDisplay? FirstDisplay(nint factory)
    {
        var enumAdapters = (delegate* unmanaged<nint, uint, nint*, int>)ComObject.Slot(factory, Dxgi.EnumAdapters1Slot);

        for (uint index = 0; ; index++)
        {
            nint adapter = 0;

            if (enumAdapters(factory, index, &adapter) < 0)
            {
                return null;
            }

            try
            {
                var display = FirstOutput(adapter);

                if (display is not null)
                {
                    return display;
                }
            }
            finally
            {
                ComObject.Release(adapter);
            }
        }
    }

    private static HdrDisplay? FirstOutput(nint adapter)
    {
        nint output = 0;
        nint output6 = 0;

        if (((delegate* unmanaged<nint, uint, nint*, int>)ComObject.Slot(adapter, Dxgi.EnumOutputsSlot))(adapter, 0, &output) < 0)
        {
            return null;
        }

        try
        {
            Guid id = Dxgi.Output6Id;

            if (((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(output, ComObject.QueryInterfaceSlot))(output, &id, &output6) < 0)
            {
                return null;
            }

            return Describe(output6);
        }
        finally
        {
            ComObject.Release(output6);
            ComObject.Release(output);
        }
    }

    private static HdrDisplay? Describe(nint output6)
    {
        byte* desc = stackalloc byte[Dxgi.OutputDesc1Size];

        if (((delegate* unmanaged<nint, byte*, int>)ComObject.Slot(output6, Dxgi.GetDesc1Slot))(output6, desc) < 0)
        {
            return null;
        }

        bool hdr = *(uint*)(desc + Dxgi.ColorSpaceOffset) == Dxgi.ColorSpaceHdr10;
        float min = *(float*)(desc + Dxgi.MinLuminanceOffset);
        float max = *(float*)(desc + Dxgi.MaxLuminanceOffset);
        float fullFrame = *(float*)(desc + Dxgi.MaxFullFrameLuminanceOffset);

        return new HdrDisplay(hdr, min, max, fullFrame);
    }
}
