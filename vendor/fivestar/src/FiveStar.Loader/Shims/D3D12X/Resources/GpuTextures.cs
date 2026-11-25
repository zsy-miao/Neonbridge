// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.States;
using FiveStar.Loader.Shims.Xg;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Desktop textures for console memory (docs/gpu-resources.md): created for slot 133 or for a slot-70 view, registered
/// by address, and filled from the game's bytes through their XG layout at creation and whenever a refresh finds the
/// bytes changed. GPU-written textures (render target, depth-stencil, UAV) are never filled from memory.
/// </summary>
internal sealed unsafe class GpuTextures : IDisposable
{
    /// <summary>IUnknown::AddRef and ID3D12Device::CreateCommittedResource (d3d12.h).</summary>
    private const int AddRefSlot = 1;
    private const int DesktopCreateCommittedResourceSlot = 27;

    /// <summary>D3D12_HEAP_TYPE_DEFAULT and D3D12_HEAP_FLAG_NONE.</summary>
    private const int DefaultHeapType = 1;
    private const int NoHeapFlags = 0;

    /// <summary>D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | ALLOW_DEPTH_STENCIL | ALLOW_UNORDERED_ACCESS: the GPU writes the texture.</summary>
    private const int GpuWrittenFlags = 0x7;

    /// <summary>D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE | NON_PIXEL_SHADER_RESOURCE, where a texture made for a view rests.</summary>
    private const int ShaderResourceState = 0xC0;

    /// <summary>Failed fills logged one by one; the counters count them all.</summary>
    private const long LoggedFailures = 8;

    private static readonly Guid ResourceIid = new("696442be-a72e-4059-bc79-5b5c98040fad");

    private readonly object _watchGate = new();
    private readonly GameMemorySource _source;
    private readonly DesktopResourceDescriptions _descriptions;
    private readonly LoaderLog _log;
    private GpuTexture[] _watched = [];
    private long _checkedFrame = -1;

    public GpuTextures(GameMemorySource source, DesktopResourceDescriptions descriptions, LoaderLog log)
    {
        _source = source;
        _descriptions = descriptions;
        _log = log;
    }

    public GpuTextureRegistry Registry { get; } = GpuTextureRegistry.OfQueues();

    public GpuResourceCounters Counters { get; } = new();

    public GpuTextureUploader Uploader { get; } = new();

    /// <summary>The shared heaps of placed read-only textures (<c>--placed-textures</c>).</summary>
    public PlacedTextureHeaps Placed { get; } = new();

    /// <summary>
    /// Slot 133: the desktop texture for <paramref name="address"/> in its home state <paramref name="state"/>
    /// (<see cref="ResourceStates.Home"/> of <paramref name="initialLayout"/>), registered and, unless GPU-written,
    /// filled from memory. Returns the HRESULT; <paramref name="desktop"/> is the caller's reference.
    /// </summary>
    public int CreatePlaced(nint device, ulong address, ResourceDescription* description, int initialLayout, void* clear, out nint desktop, out int state)
    {
        var desc = _descriptions.ToDesktop(device, description);
        state = ResourceStates.Home(desc.Dimension, desc.Flags, BarrierLayouts.ToState(initialLayout));
        byte* clearCopy = stackalloc byte[DepthViewDescriptions.ClearValueSize];
        bool readOnly = (description->Flags & GpuWrittenFlags) == 0;
        int hr = CreateTexture(device, &desc, state, DepthViewDescriptions.ClearValue(clear, clearCopy, desc.Format), readOnly, out desktop);
        Counters.Add(GpuResourceEvent.PlacedTexture);

        if (HResult.Failed(hr))
        {
            return hr;
        }

        bool gpuWritten = (description->Flags & GpuWrittenFlags) != 0;
        var texture = Register(address, description, desktop, desc.Format, state, gpuWritten);
        Refresh(device, texture);

        return hr;
    }

    /// <summary>
    /// A read-only texture made for a slot-70 view of <paramref name="viewFormat"/> over readable memory; null when the
    /// memory is not readable or the desktop refuses the desc.
    /// </summary>
    public GpuTexture? CreateForView(nint device, ulong address, ResourceDescription* description, int viewFormat)
    {
        XgLayoutBuilder.Build(ToXg(description), out var layout);

        if (_source.Find(address, layout.Size) == null)
        {
            return null;
        }

        // Read-only: without the GPU-written flags, so a render-target format keeps its own bytes (no substitute).
        var readOnly = *description;
        readOnly.Flags &= ~GpuWrittenFlags;
        var desc = _descriptions.ToDesktop(device, &readOnly);
        desc.Format = ViewFormats.ForTexture(desc.Format, viewFormat);

        if (HResult.Failed(CreateTexture(device, &desc, ShaderResourceState, null, readOnly: true, out nint desktop)))
        {
            return null;
        }

        var texture = Register(address, description, desktop, desc.Format, ShaderResourceState, false);
        ComObject.Release(desktop);
        Refresh(device, texture);

        return texture;
    }

    /// <summary>Fills <paramref name="texture"/> from memory when it is not GPU-written, its memory is readable and its bytes changed.</summary>
    public void Refresh(nint device, GpuTexture texture)
    {
        if (texture.GpuWritten || texture.Layout.PlaneCount != 1)
        {
            return;
        }

        if (texture.CopiedInto)
        {
            Counters.Add(GpuResourceEvent.UploadKeptCopy);

            return;
        }

        byte* source = _source.Find(texture.Address, texture.Size);

        if (source == null)
        {
            return;
        }

        // A watched texture's memory can be unmapped by another thread while it is read every frame: a plain read
        // faulted (an AccessViolation the guard cannot catch, boot v3), so its bytes are copied through the kernel first.
        byte[]? copy = null;

        if (texture.Watched)
        {
            copy = ReadThroughKernel(source, texture.Size);

            if (copy is null)
            {
                return;
            }
        }

        fixed (byte* copied = copy)
        {
            if (copied != null)
            {
                source = copied;
            }

            Fill(device, texture, source);
        }
    }

    private void Fill(nint device, GpuTexture texture, byte* source)
    {
        ulong hash = ContentHash.Of(source, texture.Size);

        if (hash == texture.UploadedHash)
        {
            Counters.Add(GpuResourceEvent.UploadSkipped);

            return;
        }

        if (Uploader.Upload(device, texture, source))
        {
            texture.UploadedHash = hash;
            Counters.Add(GpuResourceEvent.Upload);

            return;
        }

        var failure = Uploader.Failure;

        if (failure is not null && Counters.Add(GpuResourceEvent.UploadFailed) <= LoggedFailures)
        {
            _log.Warn($"[d3d12_xs] texture at 0x{texture.Address:x} ({texture.Size} bytes) not filled from memory: {failure}");
        }
    }

    /// <summary>
    /// A GPU copy writes <paramref name="desktop"/>, the texture registered at <paramref name="address"/>: from now on
    /// its memory no longer refreshes it (victory-gta6 keeps a host-written texture the same way).
    /// </summary>
    [HotPath]
    public void MarkCopiedInto(ulong address, nint desktop)
    {
        var texture = Registry.Find(address);

        if (texture is not null && texture.Address == address && texture.Desktop == desktop)
        {
            texture.CopiedInto = true;
        }
    }

    /// <summary>
    /// The game mapped the texture at <paramref name="address"/> (resource Map): the CPU writes its bytes whenever it
    /// likes (video frames decoded into it, for one), so from now on it is checked once a frame.
    /// </summary>
    public void Watch(ulong address)
    {
        var texture = Registry.Find(address);

        if (texture is null || texture.GpuWritten || texture.Watched || texture.Address != address)
        {
            return;
        }

        lock (_watchGate)
        {
            if (texture.Watched)
            {
                return;
            }

            texture.Watched = true;
            Volatile.Write(ref _watched, [.. _watched, texture]);
        }

        _log.Detail($"  [d3d12_xs] texture at 0x{address:x} mapped by the CPU: checked for new bytes every frame ({_watched.Length} such)");
    }

    /// <summary>
    /// Once per <paramref name="frame"/> (the present count): releases retired textures past their grace period and
    /// refreshes the watched textures still registered.
    /// </summary>
    public void RefreshWatched(nint device, long frame)
    {
        if (Volatile.Read(ref _checkedFrame) == frame || Interlocked.Exchange(ref _checkedFrame, frame) == frame)
        {
            return;
        }

        Registry.Advance(frame);

        foreach (var texture in Volatile.Read(ref _watched))
        {
            if (Registry.Find(texture.Address) == texture)
            {
                Refresh(device, texture);
            }
        }
    }

    public void Dispose()
    {
        Registry.Dispose();
        Uploader.Dispose();
    }

    private GpuTexture Register(ulong address, ResourceDescription* description, nint desktop, int format, int state, bool gpuWritten)
    {
        XgLayoutBuilder.Build(ToXg(description), out var layout);
        var texture = new GpuTexture(address, *description, desktop, format, state, gpuWritten, layout);
        ((delegate* unmanaged<nint, uint>)ComObject.Slot(desktop, AddRefSlot))(desktop);
        Registry.Register(texture);

        if (_log.DetailEnabled)
        {
            string writer = gpuWritten ? "GPU-written" : "filled from memory";
            _log.Detail($"  [d3d12_xs] texture at 0x{address:x}: {description->Width}x{description->Height}x{description->DepthOrArraySize}, {description->MipLevels} mips, format {format}, layout 0x{description->Layout:x}, 0x{layout.Size:x} bytes, {writer}");
        }

        return texture;
    }

    /// <summary>The bytes at <paramref name="source"/> read through the kernel (ReadProcessMemory); null when a page went away.</summary>
    private static byte[]? ReadThroughKernel(byte* source, ulong size)
    {
        var copy = GC.AllocateUninitializedArray<byte>(checked((int)size));
        nuint done = 0;

        fixed (byte* target = copy)
        {
            if (Kernel32.ReadProcessMemory(Kernel32.GetCurrentProcess(), (nint)source, target, (nuint)size, &done) == 0 || done != (nuint)size)
            {
                return null;
            }
        }

        return copy;
    }

    /// <summary>
    /// A read-only single-sample texture placed in a shared heap with <c>--placed-textures</c> (<see cref="PlacedTextureHeaps"/>),
    /// else (or when placing fails) a committed one.
    /// </summary>
    private int CreateTexture(nint device, ResourceDescription* description, int state, void* clear, bool readOnly, out nint resource)
    {
        if (PlacedTextureHeaps.Enabled && readOnly && description->SampleCount <= 1)
        {
            int hr = Placed.Create(device, description, state, clear, out resource);

            if (!HResult.Failed(hr))
            {
                return hr;
            }

            Counters.Add(GpuResourceEvent.PlacedFailed);
        }

        return CreateCommitted(device, description, state, clear, out resource);
    }

    private static int CreateCommitted(nint device, ResourceDescription* description, int state, void* clear, out nint resource)
    {
        int* properties = stackalloc int[] { DefaultHeapType, 0, 0, 0, 0 };
        Guid iid = ResourceIid;
        nint created = 0;
        var create = (delegate* unmanaged<nint, int*, int, ResourceDescription*, int, void*, Guid*, nint*, int>)ComObject.Slot(device, DesktopCreateCommittedResourceSlot);
        int hr = create(device, properties, NoHeapFlags, description, state, clear, &iid, &created);
        resource = created;

        return hr;
    }

    /// <summary>The XG desc of a console resource desc: the same D3D12_RESOURCE_DESC fields.</summary>
    private static XgResourceDesc ToXg(ResourceDescription* description)
    {
        return new XgResourceDesc
        {
            Dimension = (uint)description->Dimension,
            Alignment = description->Alignment,
            Width = description->Width,
            Height = description->Height,
            DepthOrArraySize = description->DepthOrArraySize,
            MipLevels = description->MipLevels,
            Format = (uint)description->Format,
            SampleCount = description->SampleCount,
            SampleQuality = description->SampleQuality,
            Layout = (uint)description->Layout,
            Flags = (uint)description->Flags,
        };
    }
}
