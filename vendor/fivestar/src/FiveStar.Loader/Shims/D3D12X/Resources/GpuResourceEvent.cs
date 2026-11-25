// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>What happened to a texture or view at a console address; counted by <see cref="GpuResourceCounters"/>.</summary>
internal enum GpuResourceEvent
{
    /// <summary>Slot 133 made a texture.</summary>
    PlacedTexture,

    /// <summary>Slot 70 was called.</summary>
    View,

    /// <summary>A slot-70 view found a registered texture.</summary>
    ViewResolved,

    /// <summary>A slot-70 view made a texture from the memory at its address.</summary>
    ViewCreatedTexture,

    /// <summary>A slot-70 view describes a buffer (left a null descriptor).</summary>
    ViewBuffer,

    /// <summary>A slot-70 view got a null descriptor (no readable memory, a depth texture or a failed creation).</summary>
    ViewNull,

    /// <summary>
    /// A slot-70 view of a full-format texture asked for another member of its cast family (UNORM_SRGB on UNORM, UINT on
    /// UNORM or FLOAT) and reads in the texture's format: the sRGB decode or the integer reading is lost.
    /// </summary>
    ViewCastLost,

    /// <summary>A slot-70 view of a GPU-written texture asked for a format outside its family (an alias of its bytes) and reads in the texture's.</summary>
    ViewReinterpretLost,

    /// <summary>A texture was filled from memory.</summary>
    Upload,

    /// <summary>A refresh found the memory unchanged.</summary>
    UploadSkipped,

    /// <summary>A refresh was not done: a GPU copy wrote the texture, and its memory does not hold that.</summary>
    UploadKeptCopy,

    /// <summary>A fill failed: the desktop refused its upload buffer or queue, or the copy did not finish.</summary>
    UploadFailed,

    /// <summary>A texture could not be placed in a shared heap (<c>--placed-textures</c>) and was committed.</summary>
    PlacedFailed,
}
