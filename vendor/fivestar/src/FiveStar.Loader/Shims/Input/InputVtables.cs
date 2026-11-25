// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The three native vtables. Implemented slots point at the [UnmanagedCallersOnly] methods; every other slot is a
/// not-implemented stub named "gameinput.dll!Interface::Method" that logs its first call and returns E_NOTIMPL, or 0
/// for a method that does not return an HRESULT.
/// </summary>
internal sealed unsafe class InputVtables : IDisposable
{
    public InputVtables(UnresolvedStubs stubs, CodeHeap heap)
    {
        Input = Build(stubs, "IGameInput", GameInputLayout.InputMethods, &GameInputMethods.Fill);
        Reading = Build(stubs, "IGameInputReading", GameInputLayout.ReadingMethods, &ReadingMethods.Fill);
        Device = Build(stubs, "IGameInputDevice", GameInputLayout.DeviceMethods, &DeviceMethods.Fill);

        // One read-write to read-execute flip for all stubs.
        heap.Seal();
    }

    public nint Input { get; }

    public nint Reading { get; }

    public nint Device { get; }

    public void Dispose()
    {
        NativeMemory.Free((void*)Input);
        NativeMemory.Free((void*)Reading);
        NativeMemory.Free((void*)Device);
    }

    private static nint Build(UnresolvedStubs stubs, string name, IReadOnlyList<string> methods, delegate*<nint*, void> fill)
    {
        nint* vtable = (nint*)NativeMemory.AllocZeroed((nuint)(methods.Count * sizeof(nint)));
        delegate* unmanaged<nint, uint> addRef = &InputObjects.AddRef;
        delegate* unmanaged<nint, uint> release = &InputObjects.Release;
        vtable[GameInputLayout.AddRefSlot] = (nint)addRef;
        vtable[GameInputLayout.ReleaseSlot] = (nint)release;
        fill(vtable);

        for (int slot = 0; slot < methods.Count; slot++)
        {
            if (vtable[slot] != 0)
            {
                continue;
            }

            string method = methods[slot];
            long value = 0;

            if (GameInputLayout.HResultMethods.Contains(method))
            {
                value = HResult.NotImplemented;
            }

            vtable[slot] = stubs.Create(GameInputLayout.Module, $"{name}::{method}", value, true, false);
        }

        return (nint)vtable;
    }
}
