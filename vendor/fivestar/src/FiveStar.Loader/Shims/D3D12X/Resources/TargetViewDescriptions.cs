// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Render-target, UAV and shader-resource view descs in a format whose render targets have a substitute
/// (<see cref="TargetFormats"/>), rewritten into a caller's stack copy when the desktop resource was made in the
/// substitute; any other desc passes unchanged, and a missing one on a texture <see cref="TypelessTargets"/> made typeless gets its whole-resource desc. Only those formats cost a GetDesc. A UAV of a texture the desktop made
/// without ALLOW_UNORDERED_ACCESS becomes a null view.
/// </summary>
internal static unsafe class TargetViewDescriptions
{
    /// <summary>Bytes of D3D12_RENDER_TARGET_VIEW_DESC and D3D12_UNORDERED_ACCESS_VIEW_DESC (d3d12.h).</summary>
    public const int RenderTargetViewSize = 24;
    public const int UnorderedAccessViewSize = 40;

    /// <summary>ID3D12Resource::GetDesc (desktop order: object, then the return buffer).</summary>
    private const int GetDescSlot = 10;

    /// <summary>D3D12_SHADER_RESOURCE_VIEW_DESC.Shader4ComponentMapping.</summary>
    private const int MappingDword = 2;

    /// <summary>D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS and D3D12_UAV_DIMENSION_TEXTURE2D (d3d12.h).</summary>
    private const int AllowUnorderedAccess = 0x4;
    private const int UavTexture2D = 4;

    [HotPath]
    public static void* RenderTargetView(nint resource, void* description, byte* copy)
    {
        return Patched(resource, description, copy, RenderTargetViewSize, false, false);
    }

    [HotPath]
    public static void* UnorderedAccessView(nint resource, void* description, byte* copy)
    {
        return Patched(resource, description, copy, UnorderedAccessViewSize, false, true);
    }

    /// <summary>
    /// Whether <paramref name="resource"/> (a desktop resource or 0) takes a UAV: a texture the desktop made without
    /// ALLOW_UNORDERED_ACCESS (a depth target, a format without typed UAVs) does not, and a UAV of it removes the device.
    /// </summary>
    [HotPath]
    public static bool TakesUnorderedAccess(nint resource, out ResourceDescription desc)
    {
        desc = default;

        if (resource == 0)
        {
            return true;
        }

        ResourceDescription read;
        _ = ((delegate* unmanaged<nint, ResourceDescription*, ResourceDescription*>)ComObject.Slot(resource, GetDescSlot))(resource, &read);
        desc = read;

        return read.Dimension == ResourceDescription.BufferDimension || (read.Flags & AllowUnorderedAccess) != 0;
    }

    /// <summary>A null UAV desc in <paramref name="copy"/> of <paramref name="description"/>'s dimension, else a 2D texture's.</summary>
    [HotPath]
    public static void* NullUnorderedAccessView(void* description, byte* copy)
    {
        new Span<byte>(copy, UnorderedAccessViewSize).Clear();
        int* dwords = (int*)copy;
        dwords[0] = TargetFormats.R8G8B8A8Unorm;
        dwords[1] = description != null ? ((int*)description)[1] : UavTexture2D;

        return copy;
    }

    [HotPath]
    public static void* ShaderResourceView(nint resource, void* description, byte* copy)
    {
        return Patched(resource, description, copy, DepthViewDescriptions.ShaderResourceViewSize, true, false);
    }

    [HotPath]
    private static void* Patched(nint resource, void* description, byte* copy, int size, bool shaderView, bool unordered)
    {
        // A view without a desc of a texture made typeless (--typeless-targets) takes the format it was created in.
        if (description == null && TypelessTargets.Enabled)
        {
            void* whole = TypelessTargets.WholeView(resource, copy, size, shaderView, unordered);

            if (whole != null)
            {
                return whole;
            }
        }

        if (description == null || resource == 0 || !TargetFormats.HasSubstitute(*(int*)description))
        {
            return description;
        }

        int viewFormat = *(int*)description;
        ResourceDescription desc;
        _ = ((delegate* unmanaged<nint, ResourceDescription*, ResourceDescription*>)ComObject.Slot(resource, GetDescSlot))(resource, &desc);
        int format = TargetFormats.View(viewFormat, desc.Format);

        if (format == viewFormat)
        {
            return description;
        }

        Buffer.MemoryCopy(description, copy, size, size);
        int* dwords = (int*)copy;
        dwords[0] = format;

        if (shaderView)
        {
            dwords[MappingDword] = TargetFormats.ComponentMapping(viewFormat, dwords[MappingDword]);
        }

        return copy;
    }
}
