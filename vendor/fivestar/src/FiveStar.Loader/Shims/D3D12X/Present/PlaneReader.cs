// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>Turns a <see cref="PresentPlane"/> into a <see cref="PlaneSource"/>: unwraps the resource, takes its home state and clamps the rect.</summary>
internal static unsafe class PlaneReader
{
    /// <summary>The size shown when a plane has no resource to take it from.</summary>
    public const uint FallbackWidth = 1280;
    public const uint FallbackHeight = 720;

    /// <summary>ID3D12Resource::GetDesc (desktop order: object, then the return buffer).</summary>
    private const int GetDescSlot = 10;

    /// <summary>D3D12_RESOURCE_DIMENSION_TEXTURE2D.</summary>
    private const int Texture2D = 3;

    [HotPath]
    public static PlaneSource Read(XboxObjects objects, PresentPlane* plane, bool testPattern)
    {
        nint resource = 0;

        if (plane->Resources != null && plane->ResourceCount > 0)
        {
            resource = objects.Unwrap(plane->Resources[0]);
        }

        if (resource == 0)
        {
            var empty = new CopyBox { Right = FallbackWidth, Bottom = FallbackHeight, Back = 1 };

            return new PlaneSource(0, TransitionBarrier.RenderTargetState, empty, SwapChainFormats.Rgba8Unorm, SwapChainFormats.Srgb, false);
        }

        ResourceDescription description;
        _ = ((delegate* unmanaged<nint, ResourceDescription*, ResourceDescription*>)ComObject.Slot(resource, GetDescSlot))(resource, &description);
        var box = Clamp(plane->SourceRect, description);
        int format = SwapChainFormats.ForPlane(description.Format);
        bool copyable = !testPattern && format != SwapChainFormats.None && description.Dimension == Texture2D && description.SampleCount <= 1;

        if (format == SwapChainFormats.None)
        {
            format = SwapChainFormats.Rgba8Unorm;
        }

        return new PlaneSource(
            resource,
            HomeState(objects, plane->Resources[0]),
            box,
            format,
            SwapChainFormats.ColorSpace(format, plane->ColorSpace),
            copyable,
            SwapChainFormats.ViewFormat(description.Format),
            (uint)description.Width,
            description.Height);
    }

    /// <summary>The source rect inside the resource; the whole resource without one, or when it is empty.</summary>
    [HotPath]
    private static CopyBox Clamp(PlaneRect* rect, ResourceDescription description)
    {
        uint width = (uint)description.Width;
        uint height = description.Height;
        var box = new CopyBox { Right = width, Bottom = height, Back = 1 };

        if (rect == null || rect->Right <= rect->Left || rect->Bottom <= rect->Top || rect->Left < 0 || rect->Top < 0)
        {
            return box;
        }

        box.Left = (uint)rect->Left;
        box.Top = (uint)rect->Top;

        if ((uint)rect->Right < width)
        {
            box.Right = (uint)rect->Right;
        }

        if ((uint)rect->Bottom < height)
        {
            box.Bottom = (uint)rect->Bottom;
        }

        if (box.Left >= box.Right || box.Top >= box.Bottom)
        {
            return new CopyBox { Right = width, Bottom = height, Back = 1 };
        }

        return box;
    }

    /// <summary>The state the plane's resource rests in: its recorded home state, RENDER_TARGET for one the shim did not create.</summary>
    [HotPath]
    private static int HomeState(XboxObjects objects, nint resource)
    {
        var wrapper = objects.AsResource(resource);

        if (wrapper == null || wrapper->Dimension == States.ResourceStates.UnknownDimension)
        {
            return TransitionBarrier.RenderTargetState;
        }

        return wrapper->HomeState;
    }
}
