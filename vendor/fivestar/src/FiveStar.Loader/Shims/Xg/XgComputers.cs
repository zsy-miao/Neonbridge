// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Xg;

/// <summary>
/// Hands out XGTextureAddressComputer objects that share one vtable. Slots the game was not seen calling are stubs
/// named "xg_xs.dll!XGTextureAddressComputer::slotN" that log their first call and return E_NOTIMPL.
/// </summary>
internal sealed unsafe class XgComputers : IDisposable
{
    private const string InterfaceName = "XGTextureAddressComputer";

    /// <summary>The game calls slots up to 10; the rest leave room for calls not seen yet (a guess).</summary>
    private const int VtableSlots = 16;

    private const int AddRefSlot = 0;
    private const int ReleaseSlot = 1;
    private const int GetResourceLayoutSlot = 2;
    private const int GetResourceSizeBytesSlot = 3;
    private const int GetResourceBaseAlignmentBytesSlot = 4;
    private const int GetMipLevelOffsetBytesSlot = 5;
    private const int GetTexelElementOffsetBytesSlot = 6;
    private const int CopyIntoSubresourceSlot = 8;
    private const int CopyFromSubresourceSlot = 9;
    private const int GetResourceTilingSlot = 10;

    private readonly nint _vtable;

    public XgComputers(UnresolvedStubs stubs, CodeHeap heap)
    {
        var vtable = (nint*)NativeMemory.AllocZeroed((nuint)(VtableSlots * sizeof(nint)));
        Fill(vtable);

        for (int slot = 0; slot < VtableSlots; slot++)
        {
            if (vtable[slot] == 0)
            {
                vtable[slot] = stubs.Create(XgModule.Module, $"{InterfaceName}::slot{slot}", HResult.NotImplemented, true, false);
            }
        }

        // One read-write to read-execute flip for all stubs.
        heap.Seal();
        _vtable = (nint)vtable;
    }

    /// <summary>XGCreateTextureComputer: a new computer with one reference for the description.</summary>
    [HotPath]
    public int Create(XgResourceDesc* desc, nint* result)
    {
        if (desc == null || result == null)
        {
            return HResult.Pointer;
        }

        var computer = (XgComputerObject*)NativeMemory.AllocZeroed((nuint)sizeof(XgComputerObject));
        computer->Vtable = _vtable;
        computer->References = 1;
        XgLayoutBuilder.Build(*desc, out computer->Surface);
        *result = (nint)computer;

        return HResult.Ok;
    }

    /// <summary>XGSuggestSwizzleMode: writes the layout value (0x100 + mode) the game stores in the description.</summary>
    [HotPath]
    public static int Suggest(XgResourceDesc* desc, uint* layout)
    {
        if (desc == null || layout == null)
        {
            return HResult.Pointer;
        }

        var format = XgFormats.Find(desc->Format);
        long width = XgLayoutBuilder.Elements(XgLayoutBuilder.AtLeastOne(desc->Width), format.BlockDimension);
        long height = XgLayoutBuilder.Elements(XgLayoutBuilder.AtLeastOne(desc->Height), format.BlockDimension);
        *layout = XgSwizzleModes.ModeBase | XgSwizzleModes.Suggest(width * height * format.BytesPerElement);

        return HResult.Ok;
    }

    public void Dispose()
    {
        NativeMemory.Free((void*)_vtable);
    }

    private static void Fill(nint* vtable)
    {
        delegate* unmanaged<nint, uint> addRef = &XgComputerMethods.AddRef;
        delegate* unmanaged<nint, uint> release = &XgComputerMethods.Release;
        delegate* unmanaged<nint, byte*, int> layout = &XgComputerMethods.GetResourceLayout;

        delegate* unmanaged<nint, ulong> size = &XgComputerMethods.GetResourceSizeBytes;
        delegate* unmanaged<nint, ulong> alignment = &XgComputerMethods.GetResourceBaseAlignmentBytes;
        delegate* unmanaged<nint, uint, uint, ulong> mipOffset = &XgComputerMethods.GetMipLevelOffsetBytes;
        delegate* unmanaged<nint, uint, uint, ulong, uint, uint, uint, uint*, ulong> texelOffset = &XgComputerMethods.GetTexelElementOffsetBytes;

        delegate* unmanaged<nint, byte*, uint, uint, byte*, uint, uint, int> copyInto = &XgComputerMethods.CopyIntoSubresource;
        delegate* unmanaged<nint, byte*, uint, uint, uint, byte*, uint, uint, int> copyFrom = &XgComputerMethods.CopyFromSubresource;
        delegate* unmanaged<nint, uint*, XgPackedMipInfo*, XgTileShape*, uint*, uint, XgSubresourceTiling*, void> tiling = &XgComputerMethods.GetResourceTiling;

        vtable[AddRefSlot] = (nint)addRef;
        vtable[ReleaseSlot] = (nint)release;
        vtable[GetResourceLayoutSlot] = (nint)layout;

        vtable[GetResourceSizeBytesSlot] = (nint)size;
        vtable[GetResourceBaseAlignmentBytesSlot] = (nint)alignment;
        vtable[GetMipLevelOffsetBytesSlot] = (nint)mipOffset;
        vtable[GetTexelElementOffsetBytesSlot] = (nint)texelOffset;

        vtable[CopyIntoSubresourceSlot] = (nint)copyInto;
        vtable[CopyFromSubresourceSlot] = (nint)copyFrom;
        vtable[GetResourceTilingSlot] = (nint)tiling;
    }
}
