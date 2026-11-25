// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The title's memory hooks: the game exports <c>__XMemAllocationHooks__</c> (XMemAlloc and XMemFree routines) for
/// system components to allocate through. D3D12.X allocates a descriptor heap's CPU memory there, and the game relies
/// on it: around one heap creation (0x141cf3280) it pushes a hook that hands out memory it reserved, then breaks into
/// the debugger unless the heap's CPU start is that memory.
/// </summary>
internal sealed unsafe class TitleMemoryHooks
{
    private const string HooksExport = "__XMemAllocationHooks__";

    private readonly ImageTable _images;
    private nint _hooks;

    public TitleMemoryHooks(ImageTable images)
    {
        _images = images;
    }

    /// <summary>The title's XMemAlloc(size, attributes); 0 when no loaded image exports the hooks.</summary>
    public nint Allocate(nuint size, ulong attributes)
    {
        nint hooks = Hooks();

        if (hooks == 0)
        {
            return 0;
        }

        var allocate = (delegate* unmanaged<nuint, ulong, nint>)(*(nint*)hooks);

        return allocate(size, attributes);
    }

    /// <summary>The title's XMemFree(address, attributes), the routine after XMemAlloc in the hooks.</summary>
    public void Free(nint address, ulong attributes)
    {
        nint hooks = Hooks();

        if (hooks == 0 || address == 0)
        {
            return;
        }

        var free = (delegate* unmanaged<nint, ulong, void>)((nint*)hooks)[1];
        free(address, attributes);
    }

    private nint Hooks()
    {
        if (_hooks != 0)
        {
            return _hooks;
        }

        foreach (var image in _images.All)
        {
            var target = image.Exports?.Find(HooksExport, 0);

            if (target is not null && target.Address != 0)
            {
                _hooks = target.Address;

                return _hooks;
            }
        }

        return 0;
    }
}
