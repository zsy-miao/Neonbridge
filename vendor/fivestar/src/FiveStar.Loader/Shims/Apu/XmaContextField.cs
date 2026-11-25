// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>A bit field of a SHAPE XMA context: <paramref name="Width"/> bits at <paramref name="Shift"/> of dword <paramref name="Dword"/>.</summary>
internal readonly record struct XmaContextField(int Dword, int Shift, int Width)
{
    private uint Mask => (uint)((1UL << Width) - 1);

    public unsafe uint Read(uint* context)
    {
        return (Volatile.Read(ref context[Dword]) >> Shift) & Mask;
    }

    /// <summary>Replaces the field; the dword is stored whole, so a reader on another thread sees the old or the new value.</summary>
    public unsafe void Write(uint* context, uint value)
    {
        uint dword = Volatile.Read(ref context[Dword]);
        dword = (dword & ~(Mask << Shift)) | ((value & Mask) << Shift);
        Volatile.Write(ref context[Dword], dword);
    }
}
