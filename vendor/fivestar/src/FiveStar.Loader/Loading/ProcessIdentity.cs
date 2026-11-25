// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Loading;

/// <summary>
/// PEB.ImageBaseAddress, which code reading gs:[60h] and the real GetModuleHandle(NULL) see. The host is the managed
/// loader executable; the module shims fix the API view for every import the loader binds, and the PEB itself is
/// patched only on request (<c>--patch-peb</c>), because that changes the process for the runtime too.
/// </summary>
internal sealed unsafe class ProcessIdentity
{
    private readonly LoaderLog _log;
    private nint _original;

    public ProcessIdentity(LoaderLog log)
    {
        _log = log;
    }

    public bool PebPatched { get; private set; }

    public nint PebImageBase => *(nint*)(ProcessEnvironmentBlock.Address() + ProcessEnvironmentBlock.ImageBaseAddressOffset);

    /// <summary>Points PEB.ImageBaseAddress at <paramref name="imageBase"/>.</summary>
    public void PatchPeb(nint imageBase)
    {
        nint field = ProcessEnvironmentBlock.Address() + ProcessEnvironmentBlock.ImageBaseAddressOffset;

        if (!PebPatched)
        {
            _original = *(nint*)field;
        }

        *(nint*)field = imageBase;
        PebPatched = true;
        _log.Info($"  [loader] PEB.ImageBaseAddress 0x{_original:x} -> 0x{imageBase:x}");
    }

    public void RestorePeb()
    {
        if (!PebPatched)
        {
            return;
        }

        *(nint*)(ProcessEnvironmentBlock.Address() + ProcessEnvironmentBlock.ImageBaseAddressOffset) = _original;
        PebPatched = false;
        _log.Info($"  [loader] PEB.ImageBaseAddress restored to 0x{_original:x}");
    }
}
