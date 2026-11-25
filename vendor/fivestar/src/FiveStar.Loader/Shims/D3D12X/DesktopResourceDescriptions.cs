// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Resource descs as the desktop takes them: <see cref="ResourceDescription.ToDesktop"/>, console depth formats in their
/// desktop family (<see cref="DepthFormats.Resource"/>), ALLOW_RENDER_TARGET and ALLOW_UNORDERED_ACCESS dropped from a
/// depth-stencil texture (the console takes UAVs of depth, the desktop refuses the combination), a render-target or UAV
/// texture in its substitute format (<see cref="TargetFormats"/>), and ALLOW_UNORDERED_ACCESS dropped from a texture whose format family
/// (<see cref="FormatFamilies"/>) has no typed UAV on this GPU (the console sets it on formats such as B5G5R5A1_UNORM
/// and its D16_UNORM_S8_UINT depth, which the desktop then rejects). UAV support is asked once per format; a format the
/// runtime refuses anyway is recorded through <see cref="RefuseUav"/>.
/// </summary>
internal sealed class DesktopResourceDescriptions
{
    /// <summary>D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, ALLOW_DEPTH_STENCIL, ALLOW_UNORDERED_ACCESS and ALLOW_SIMULTANEOUS_ACCESS (d3d12.h).</summary>
    private const int AllowRenderTarget = 0x1;
    private const int AllowDepthStencil = 0x2;
    private const int AllowUnorderedAccess = 0x4;
    private const int AllowSimultaneousAccess = 0x20;

    /// <summary>The flags the desktop refuses next to ALLOW_DEPTH_STENCIL.</summary>
    private const int NotWithDepthStencil = AllowRenderTarget | AllowUnorderedAccess | AllowSimultaneousAccess;

    /// <summary>The flags that make a texture GPU-written in color.</summary>
    private const int GpuWrittenColor = AllowRenderTarget | AllowUnorderedAccess;

    /// <summary>DXGI formats the cache covers; the desktop's end below 256.</summary>
    private const int FormatCount = 256;

    private const byte Unknown = 0;
    private const byte Supported = 1;
    private const byte Unsupported = 2;

    private readonly byte[] _uavSupport = new byte[FormatCount];

    /// <summary>The desktop form of <paramref name="description"/> for <paramref name="device"/>: what resources are created with.</summary>
    public unsafe ResourceDescription ToDesktop(nint device, ResourceDescription* description)
    {
        var desktop = Prepared(description);

        if (IsTexture(desktop) && (desktop.Flags & GpuWrittenColor) != 0)
        {
            desktop.Format = Resources.TypelessTargets.Resource(TargetFormats.GpuWritten(device, desktop.Format));
        }

        DropUnsupportedUav(device, ref desktop);

        return desktop;
    }

    /// <summary>
    /// The desc an allocation-size query (slots 25 and 108) takes: the console format kept and the flags it lacks
    /// support for dropped where <see cref="ToDesktop"/> substitutes, so the game sizes its memory for the console
    /// format (B4G4R4A4: 2 bytes a texel) while the resource itself is created in the substitute.
    /// </summary>
    public unsafe ResourceDescription ToAllocationQuery(nint device, ResourceDescription* description)
    {
        var desktop = Prepared(description);

        if (IsTexture(desktop) && (desktop.Flags & AllowRenderTarget) != 0 && TargetFormats.HasSubstitute(desktop.Format) && !FormatSupport.RendersTo(device, desktop.Format))
        {
            desktop.Flags &= ~AllowRenderTarget;
        }

        DropUnsupportedUav(device, ref desktop);

        return desktop;
    }

    /// <summary>
    /// Records that the runtime refused a UAV texture of <paramref name="description"/>'s format although its family
    /// reports typed UAV support; true when that changes the desktop desc.
    /// </summary>
    public unsafe bool RefuseUav(ResourceDescription* description)
    {
        var desktop = Prepared(description);
        int format = desktop.Format;
        bool texture = IsTexture(desktop) && (desktop.Flags & AllowUnorderedAccess) != 0;

        if (!texture || (uint)format >= FormatCount || Volatile.Read(ref _uavSupport[format]) == Unsupported)
        {
            return false;
        }

        Volatile.Write(ref _uavSupport[format], Unsupported);

        return true;
    }

    private static unsafe ResourceDescription Prepared(ResourceDescription* description)
    {
        var desktop = description->ToDesktop();
        desktop.Format = DepthFormats.Resource(desktop.Format);

        if ((desktop.Flags & AllowDepthStencil) != 0)
        {
            desktop.Flags &= ~NotWithDepthStencil;
        }

        return desktop;
    }

    private static bool IsTexture(in ResourceDescription description)
    {
        return description.Dimension != ResourceDescription.BufferDimension;
    }

    private void DropUnsupportedUav(nint device, ref ResourceDescription desktop)
    {
        if (IsTexture(desktop) && (desktop.Flags & AllowUnorderedAccess) != 0 && !HasTypedUav(device, desktop.Format))
        {
            desktop.Flags &= ~AllowUnorderedAccess;
        }
    }

    private bool HasTypedUav(nint device, int format)
    {
        if ((uint)format >= FormatCount)
        {
            return true;
        }

        byte known = Volatile.Read(ref _uavSupport[format]);

        if (known != Unknown)
        {
            return known == Supported;
        }

        bool supported = false;

        foreach (int member in FormatFamilies.Members(format))
        {
            supported |= (FormatSupport.Support1(device, member) & FormatSupport.TypedUnorderedAccess) != 0;
        }

        byte state = supported ? Supported : Unsupported;
        Volatile.Write(ref _uavSupport[format], state);

        return supported;
    }
}
