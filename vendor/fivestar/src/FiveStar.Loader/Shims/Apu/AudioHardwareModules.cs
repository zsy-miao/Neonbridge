// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// chad.dll (XDsp, the audio DSP, <see cref="XDspShim"/>) and xapu.dll (the APU command queue, <see cref="XApuShim"/>).
/// Neither is in the public GDK headers. A function of either module without a shim returns E_NOTIMPL.
/// </summary>
[ShimModule(XDspModule, UnresolvedReturn = HResult.NotImplemented)]
[ShimModule(XApuModule, UnresolvedReturn = HResult.NotImplemented)]
internal static partial class AudioHardwareModules
{
    public const string XDspModule = "chad.dll";
    public const string XApuModule = "xapu.dll";
}
