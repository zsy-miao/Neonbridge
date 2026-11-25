// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.States;

/// <summary>
/// The legacy D3D12_RESOURCE_STATES a desktop resource rests in between command lists (its home state). The console
/// game creates render targets in other layouts (UNORDERED_ACCESS, SHADER_RESOURCE) and clears and draws to them
/// without a barrier, so a texture the GPU writes rests in the state of its first write flag: depth-stencil, then
/// render target, then unordered access. Every other resource rests in the state it was created in; a write state
/// without the matching flag (ALLOW_UNORDERED_ACCESS dropped for the format) becomes COMMON.
/// </summary>
internal static class ResourceStates
{
    /// <summary>D3D12_RESOURCE_STATES (d3d12.h).</summary>
    public const int Common = 0;
    public const int RenderTarget = 0x4;
    public const int UnorderedAccess = 0x8;
    public const int DepthWrite = 0x10;
    public const int DepthRead = 0x20;
    public const int NonPixelShaderResource = 0x40;
    public const int PixelShaderResource = 0x80;
    public const int CopyDest = 0x400;
    public const int CopySource = 0x800;
    public const int ResolveDest = 0x1000;
    public const int ResolveSource = 0x2000;
    public const int GenericRead = 0xAC3;
    private const int VertexAndConstantBuffer = 0x1;
    private const int IndirectArgument = 0x200;
    private const int AccelerationStructure = 0x400000;

    /// <summary>D3D12_COMMAND_LIST_TYPE_COMPUTE and _COPY.</summary>
    private const int ComputeList = 2;
    private const int CopyList = 3;

    /// <summary>D3D12_RESOURCE_DIMENSION_UNKNOWN and _BUFFER.</summary>
    public const int UnknownDimension = 0;
    public const int BufferDimension = 1;

    /// <summary>D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, _DEPTH_STENCIL and _UNORDERED_ACCESS.</summary>
    private const int AllowRenderTarget = 0x1;
    private const int AllowDepthStencil = 0x2;
    private const int AllowUnorderedAccess = 0x4;

    /// <summary>
    /// The home state of a desktop resource of <paramref name="dimension"/> and desktop <paramref name="flags"/> the
    /// game asked to create in <paramref name="created"/>.
    /// </summary>
    public static int Home(int dimension, int flags, int created)
    {
        if (dimension == BufferDimension)
        {
            return created;
        }

        if ((flags & AllowDepthStencil) != 0)
        {
            return DepthWrite;
        }

        if ((flags & AllowRenderTarget) != 0)
        {
            return RenderTarget;
        }

        if ((flags & AllowUnorderedAccess) != 0)
        {
            return UnorderedAccess;
        }

        if ((created & (RenderTarget | UnorderedAccess | DepthWrite | DepthRead)) != 0)
        {
            return Common;
        }

        return created;
    }

    /// <summary>Records the dimension, home state and desktop flags on a resource wrapper (null is ignored).</summary>
    public static unsafe void Record(XboxResourceObject* resource, int dimension, int home, int flags)
    {
        if (resource == null)
        {
            return;
        }

        resource->Dimension = dimension;
        resource->HomeState = home;
        resource->Flags = flags;
    }

    /// <summary>
    /// The states a barrier on a desktop list of D3D12_COMMAND_LIST_TYPE <paramref name="listType"/> may name: every
    /// state on a direct list; on a compute list the compute ones (VERTEX_AND_CONSTANT_BUFFER, UNORDERED_ACCESS,
    /// NON_PIXEL_SHADER_RESOURCE, INDIRECT_ARGUMENT, COPY_DEST, COPY_SOURCE, RAYTRACING_ACCELERATION_STRUCTURE); on a
    /// copy list COPY_DEST and COPY_SOURCE. A barrier naming any other bit makes the desktop Close fail with
    /// E_INVALIDARG, and every later Reset of that list fails the same way, on any allocator.
    /// </summary>
    public static int Legal(int listType)
    {
        return listType switch
        {
            ComputeList => VertexAndConstantBuffer | UnorderedAccess | NonPixelShaderResource | IndirectArgument | CopyDest | CopySource | AccelerationStructure,
            CopyList => CopyDest | CopySource,
            _ => ~0,
        };
    }

    /// <summary>
    /// <paramref name="state"/> without the write states the desktop <paramref name="flags"/> do not allow (a flag the
    /// shim dropped, such as UAV on depth); COMMON when nothing is left. A transition to them is error #523.
    /// </summary>
    [HotPath]
    public static int Allowed(int state, int flags)
    {
        int allowed = state;

        if ((flags & AllowRenderTarget) == 0)
        {
            allowed &= ~RenderTarget;
        }

        if ((flags & AllowUnorderedAccess) == 0)
        {
            allowed &= ~UnorderedAccess;
        }

        if ((flags & AllowDepthStencil) == 0)
        {
            allowed &= ~(DepthWrite | DepthRead);
        }

        return allowed;
    }
}
