// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Graphics.Pipelines;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using FiveStar.Loader.Shims.D3D12X.Resources;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The command-list slots that bind (docs/d3d12x-shim.md, Binding). Slots 33 to 46 are the desktop root methods four
/// slots later; slot 32 is SetDescriptorHeaps (call at 0x144e4cac2 with the main view and sampler heaps). Root
/// arguments go to the list's user data and reach the GPU as one root CBV (or root constants, <see cref="UserDataConstants"/>)
/// per draw or dispatch that follows a change.
/// </summary>
internal static unsafe class CommandListBindingMethods
{
    public const int CreateRootSignatureSlot = 16;
    public const int ResetSlot = 10;
    public const int DrawInstancedSlot = 12;
    public const int DrawIndexedInstancedSlot = 13;
    public const int DispatchSlot = 14;
    public const int SetDescriptorHeapsSlot = 32;
    public const int SetComputeRootSignatureSlot = 33;
    public const int SetGraphicsRootSignatureSlot = 34;
    public const int SetComputeRootDescriptorTableSlot = 35;
    public const int SetComputeRoot32BitConstantSlot = 37;
    public const int SetComputeRoot32BitConstantsSlot = 39;
    public const int FirstRootAddressSlot = 41;
    public const int LastRootAddressSlot = 45;
    public const int ExecuteIndirectSlot = 61;

    /// <summary>Command-list slot 139: Reset with a third argument (0x142474cce).</summary>
    public const int ResetXSlot = 139;

    /// <summary>ID3D12GraphicsCommandList in the desktop vtable (d3d12.h).</summary>
    private const int DesktopReset = 10;
    private const int DesktopDrawInstanced = 12;
    private const int DesktopDrawIndexedInstanced = 13;
    private const int DesktopDispatch = 14;
    private const int DesktopSetDescriptorHeaps = 28;
    private const int DesktopSetComputeRootSignature = 29;
    private const int DesktopSetGraphicsRootSignature = 30;
    private const int DesktopSetComputeRootDescriptorTable = 31;
    private const int DesktopSetGraphicsRootDescriptorTable = 32;
    private const int DesktopSetComputeRoot32BitConstants = 35;
    private const int DesktopSetGraphicsRoot32BitConstants = 36;
    private const int DesktopSetComputeRootConstantBufferView = 37;
    private const int DesktopSetGraphicsRootConstantBufferView = 38;
    private const int DesktopExecuteIndirect = 59;
    private const int DesktopSetPipelineState = 25;

    /// <summary>Root parameters of BindlessRootSignature: the user data and the mesh fallback table.</summary>
    private const uint UserDataParameter = BindlessRootSignature.UserDataParameter;
    private const uint FallbackTableParameter = BindlessRootSignature.FallbackTableParameter;

    /// <summary>The game binds at most a view and a sampler heap.</summary>
    private const int MaxHeaps = 2;

    private static XboxBindings Bindings => ShimContext.Runtime.D3D12X.Bindings;

    /// <summary>Device slot 16: the desktop root signature, whose layout is kept for the lists.</summary>
    [UnmanagedCallersOnly]
    public static int CreateRootSignature(XboxObject* self, uint nodeMask, void* blob, nuint size, Guid* iid, nint* rootSignature)
    {
        try
        {
            nint inner = self->Inner;
            var create = (delegate* unmanaged<nint, uint, void*, nuint, Guid*, nint*, int>)ComObject.Slot(inner, CreateRootSignatureSlot);
            int hr = create(inner, nodeMask, blob, size, iid, rootSignature);

            if (!HResult.Failed(hr) && rootSignature != null && blob != null && !Bindings.Layouts.Register(*rootSignature, new ReadOnlySpan<byte>(blob, (int)size)))
            {
                Bindings.Warnings.Warn("root-layout", "[d3d12_xs] CreateRootSignature: a blob without a readable RTS0 part; its root arguments are dropped");
            }

            return hr;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.CreateRootSignature");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static int Reset(XboxObject* self, nint allocator, nint pipelineState)
    {
        try
        {
            return ResetList(self, allocator, pipelineState);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.Reset");

            return HResult.Fail;
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static int ResetX(XboxObject* self, nint allocator, nint pipelineState, uint flags)
    {
        try
        {
            return ResetList(self, allocator, pipelineState);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.ResetX");

            return HResult.Fail;
        }
    }

    /// <summary>Slot 32: the desktop heaps behind the game's heaps.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetDescriptorHeaps(XboxObject* self, uint count, nint* heaps)
    {
        try
        {
            var objects = ShimContext.Runtime.D3D12X.Objects;
            nint* desktop = stackalloc nint[MaxHeaps];
            uint used = count;

            if (used > MaxHeaps)
            {
                used = MaxHeaps;
            }

            for (uint i = 0; i < used; i++)
            {
                desktop[i] = objects.Unwrap(heaps[i]);
            }

            BindHeaps(self, used, desktop);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetDescriptorHeaps");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetComputeRootSignature(XboxObject* self, nint rootSignature)
    {
        try
        {
            SetRootSignature(self, rootSignature, true);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetComputeRootSignature");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetGraphicsRootSignature(XboxObject* self, nint rootSignature)
    {
        try
        {
            SetRootSignature(self, rootSignature, false);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetGraphicsRootSignature");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetComputeRootDescriptorTable(XboxObject* self, uint parameter, ulong handle)
    {
        try
        {
            var bindings = CommandListBindings.Of(self);
            UserData.Write(bindings, true, parameter, 0, TableAddress(bindings, true, parameter, handle));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetComputeRootDescriptorTable");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetGraphicsRootDescriptorTable(XboxObject* self, uint parameter, ulong handle)
    {
        try
        {
            var bindings = CommandListBindings.Of(self);
            UserData.Write(bindings, false, parameter, 0, TableAddress(bindings, false, parameter, handle));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetGraphicsRootDescriptorTable");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetComputeRoot32BitConstant(XboxObject* self, uint parameter, uint value, uint offset)
    {
        try
        {
            UserData.Write(CommandListBindings.Of(self), true, parameter, offset, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetComputeRoot32BitConstant");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetGraphicsRoot32BitConstant(XboxObject* self, uint parameter, uint value, uint offset)
    {
        try
        {
            UserData.Write(CommandListBindings.Of(self), false, parameter, offset, value);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetGraphicsRoot32BitConstant");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetComputeRoot32BitConstants(XboxObject* self, uint parameter, uint count, uint* values, uint offset)
    {
        try
        {
            WriteConstants(CommandListBindings.Of(self), true, parameter, count, values, offset);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetComputeRoot32BitConstants");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetGraphicsRoot32BitConstants(XboxObject* self, uint parameter, uint count, uint* values, uint offset)
    {
        try
        {
            WriteConstants(CommandListBindings.Of(self), false, parameter, count, values, offset);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetGraphicsRoot32BitConstants");
        }
    }

    /// <summary>
    /// Root CBV, SRV or UAV: the console GPU address goes to the parameter's SGPR pair as the game passes it, and the
    /// memory behind it gets a desktop buffer if it has none, so the address map holds it (<see cref="GameMemoryViews"/>).
    /// </summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetComputeRootAddress(XboxObject* self, uint parameter, ulong address)
    {
        try
        {
            ShimContext.Runtime.D3D12X.GameMemory.TryFind(address, out _);
            TransientPoolWatch.Note(address, ListActivity.Of(self)->Pipeline, true);
            UserData.WriteAddress(CommandListBindings.Of(self), true, parameter, address);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetComputeRootAddress");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void SetGraphicsRootAddress(XboxObject* self, uint parameter, ulong address)
    {
        try
        {
            ShimContext.Runtime.D3D12X.GameMemory.TryFind(address, out _);
            TransientPoolWatch.Note(address, ListActivity.Of(self)->Pipeline, false);
            UserData.WriteAddress(CommandListBindings.Of(self), false, parameter, address);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.SetGraphicsRootAddress");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void DrawInstanced(XboxObject* self, uint vertices, uint instances, uint firstVertex, uint firstInstance)
    {
        try
        {
            if (PipelinePending(self, draw: true))
            {
                return;
            }

            if (DepthTwins.Any)
            {
                CopyTwin(self);
            }

            Flush(self, false);
            ListActivity.Of(self)->Draws++;
            GpuBreadcrumbs.Before(self, MarkedKind.Draw, vertices, instances, 0);

            ShimContext.Runtime.D3D12X.Rectangles.BeforeDraw(self);
            ShimContext.Runtime.D3D12X.DrawTrace?.Draw(self, vertices, instances);
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, uint, uint, uint, void>)ComObject.Slot(inner, DesktopDrawInstanced))(inner, vertices, instances, firstVertex, firstInstance);
            ShimContext.Runtime.D3D12X.DrawTrace?.AfterDraw(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.DrawInstanced");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void DrawIndexedInstanced(XboxObject* self, uint indices, uint instances, uint firstIndex, int baseVertex, uint firstInstance)
    {
        try
        {
            if (PipelinePending(self, draw: true))
            {
                return;
            }

            if (DepthTwins.Any)
            {
                CopyTwin(self);
            }

            Flush(self, false);
            ListActivity.Of(self)->Draws++;
            GpuBreadcrumbs.Before(self, MarkedKind.Draw, indices, instances, 0);

            ShimContext.Runtime.D3D12X.Rectangles.BeforeDraw(self);
            ShimContext.Runtime.D3D12X.DrawTrace?.Draw(self, indices, instances);
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, uint, uint, int, uint, void>)ComObject.Slot(inner, DesktopDrawIndexedInstanced))(inner, indices, instances, firstIndex, baseVertex, firstInstance);
            ShimContext.Runtime.D3D12X.DrawTrace?.AfterDraw(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.DrawIndexedInstanced");
        }
    }

    [HotPath]
    [UnmanagedCallersOnly]
    public static void Dispatch(XboxObject* self, uint x, uint y, uint z)
    {
        try
        {
            if (ComputeChainBroken(self, producer: true))
            {
                return;
            }

            Flush(self, true);
            ListActivity.Of(self)->Dispatches++;
            GpuBreadcrumbs.Before(self, MarkedKind.Dispatch, x, y, z);
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, uint, uint, void>)ComObject.Slot(inner, DesktopDispatch))(inner, x, y, z);

            if (RtStandIns.Enabled)
            {
                RtStandIns.AfterDispatch(self, RectangleListState.Of(self)->Pipeline);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.Dispatch");
        }
    }

    /// <summary>Slot 61: (signature, max count, argument buffer, offset, count buffer, offset) with wrapped objects.</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static void ExecuteIndirect(XboxObject* self, nint signature, uint maxCount, nint arguments, ulong argumentOffset, nint count, ulong countOffset)
    {
        try
        {
            bool consumer = EmptyIndirectAfterSkip && CommandListBindings.Of(self)->ProducerSkipped != 0;

            if (ComputeChainBroken(self, CommandSignatures.Clampable(signature).Kind == IndirectClamp.DispatchKind))
            {
                return;
            }

            // Its arguments, counts and offsets may come from the skipped pass: stale, so it runs nothing.
            if (consumer)
            {
                ShimContext.Runtime.D3D12X.Pipelines?.StandIns.SkippedAfterChain();

                return;
            }

            FlushForIndirect(self, signature);
            ListActivity.Of(self)->Indirect++;
            GpuBreadcrumbs.BeforeIndirect(self, signature, maxCount, ArgumentAddress(arguments, argumentOffset));
            var trace = ShimContext.Runtime.D3D12X.DrawTrace;
            trace?.Indirect(self, maxCount, ArgumentAddress(arguments, argumentOffset), CommandSignatures.Clampable(signature).Stride, count != 0);

            var objects = ShimContext.Runtime.D3D12X.Objects;
            nint inner = self->Inner;
            var execute = (delegate* unmanaged<nint, nint, uint, nint, ulong, nint, ulong, void>)ComObject.Slot(inner, DesktopExecuteIndirect);

            nint argumentBuffer = BufferCopies.Desktop(arguments, ref argumentOffset);
            nint countBuffer = BufferCopies.Desktop(count, ref countOffset);

            if (NullAddressCheck.Enabled && IndirectUnsafe(self, signature, maxCount, argumentBuffer, argumentOffset, countBuffer, countOffset))
            {
                return;
            }

            if (IndirectClamp.Record(self, signature, maxCount, argumentBuffer, argumentOffset, countBuffer, countOffset, out nint scratch, out ulong scratchOffset))
            {
                RestoreCompute(self);
                inner = self->Inner;
                execute = (delegate* unmanaged<nint, nint, uint, nint, ulong, nint, ulong, void>)ComObject.Slot(inner, DesktopExecuteIndirect);
                execute(inner, objects.Unwrap(signature), maxCount, scratch, scratchOffset, countBuffer, countOffset);
                IndirectClamp.Done(self, scratch);
                trace?.AfterIndirect(self);

                return;
            }

            execute(inner, objects.Unwrap(signature), maxCount, argumentBuffer, argumentOffset, countBuffer, countOffset);
            trace?.AfterIndirect(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "CommandListBindingMethods.ExecuteIndirect");
        }
    }

    /// <summary>
    /// <c>--check-null-addresses</c>: whether an ExecuteIndirect's desktop argument or count buffer is missing, at a low
    /// address or read past its end (<see cref="NullAddressCheck"/>); true drops it.
    /// </summary>
    private static bool IndirectUnsafe(XboxObject* self, nint signature, uint maxCount, nint arguments, ulong argumentOffset, nint count, ulong countOffset)
    {
        ulong bytes = (ulong)CommandSignatures.Clampable(signature).Stride * maxCount;
        string? problem = NullAddressCheck.Indirect(arguments, argumentOffset, bytes, count, countOffset);

        return problem is not null && NullAddressCheck.Drop(NullAddressKind.IndirectBuffer, self, $"ExecuteIndirect of {maxCount}: {problem}");
    }

    /// <summary>Before an ExecuteIndirect: a depth twin's copy for draws (<see cref="CopyTwin"/>), then both bind points' user data.</summary>
    [HotPath]
    private static void FlushForIndirect(XboxObject* self, nint signature)
    {
        if (DepthTwins.Any && CommandSignatures.Clampable(signature).Kind != IndirectClamp.DispatchKind)
        {
            CopyTwin(self);
        }

        Flush(self, false);
        Flush(self, true);
    }

    /// <summary>
    /// <c>--depth-twins</c>, before a draw: when the bound depth-stencil view's texture has a twin that a pass wrote since,
    /// the twin's values go into the depth (<see cref="DepthTwinCopy"/>), then the list's graphics state is set again
    /// (heaps, input, targets, viewports, root signature, user data, pipeline).
    /// </summary>
    private static void CopyTwin(XboxObject* self)
    {
        var replay = ListReplayState.Of(self);

        if ((replay->Set & ListReplayState.DepthStencilBit) == 0)
        {
            return;
        }

        nint view = (nint)replay->DepthStencil;
        var twin = DepthTwins.OfView(view, out int format);

        if (twin is null || twin.ViewHandle == 0 || !twin.TakeWritten())
        {
            return;
        }

        var d3d12x = ShimContext.Runtime.D3D12X;
        nint heap = d3d12x.Bindings.Window.ViewHeap;
        ulong table = DesktopCalls.HeapGpuStart(heap) + (ulong)(twin.ViewHandle - DesktopCalls.HeapCpuStart(heap));
        int state = d3d12x.ListStates.For(self).Current(twin.Depth, twin.Home);
        nint list = self->Inner;

        if (!DepthTwinCopy.Record(d3d12x.Objects.DesktopDevice, list, twin, view, format, state, table))
        {
            return;
        }

        ListSplits.RestoreGraphics(replay, list);

        if (CommandListBindings.Of(self)->GraphicsRoot != 0)
        {
            SetBindlessRoot(self, false);
        }

        var rectangles = RectangleListState.Of(self);
        rectangles->VariantBound = 0;

        if (CommandListBindings.Of(self)->PipelinePending == 0 && rectangles->Pipeline != 0)
        {
            ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, DesktopSetPipelineState))(list, rectangles->Pipeline);
        }
    }

    /// <summary>After the argument clamp's dispatch: the list's compute root signature, user data and pipeline again.</summary>
    [HotPath]
    internal static void RestoreCompute(XboxObject* self)
    {
        if (CommandListBindings.Of(self)->ComputeRoot != 0)
        {
            SetBindlessRoot(self, true);
            Flush(self, true);
        }

        var rectangles = RectangleListState.Of(self);
        rectangles->VariantBound = 0;

        if (CommandListBindings.Of(self)->PipelinePending == 0 && rectangles->Pipeline != 0)
        {
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(inner, DesktopSetPipelineState))(inner, rectangles->Pipeline);
        }

        if (rectangles->Topology == PrimitiveTopologies.RectangleList)
        {
            ShimContext.Runtime.D3D12X.Rectangles.BeforeDraw(self);
        }
    }

    /// <summary>The console address of an ExecuteIndirect argument, 0 for a buffer without one.</summary>
    [HotPath]
    private static ulong ArgumentAddress(nint arguments, ulong offset)
    {
        var resource = ShimContext.Runtime.D3D12X.Objects.AsResource(arguments);

        if (resource == null || resource->GameAddress == 0)
        {
            return 0;
        }

        return resource->GameAddress + offset;
    }

    /// <summary>
    /// True while the list's pipeline is a stand-in the driver has not built: its draw or dispatch is skipped and
    /// counted; otherwise it is counted as recorded.
    /// </summary>
    [HotPath]
    private static bool PipelinePending(XboxObject* self, bool draw)
    {
        var standIns = ShimContext.Runtime.D3D12X.Pipelines?.StandIns;
        var activity = ListActivity.Of(self);
        var trace = ShimContext.Runtime.D3D12X.DrawTrace;

        if (CommandListBindings.Of(self)->PipelinePending == 0)
        {
            standIns?.Recorded(activity->Skipped);
            activity->Skipped = 0;
            trace?.Census(self, RectangleListState.Of(self)->Pipeline, skipped: false);

            // --hold-passes / --stub-passes: recorded work of a held or stubbed pass is skipped all the same.
            return PassControls.Rules.Enabled && PassControls.Intercept(self, RectangleListState.Of(self)->Pipeline, draw);
        }

        trace?.Census(self, activity->StandIn, skipped: true);

        if (PassControls.Rules.Enabled && standIns is not null)
        {
            PassControls.Skipped(standIns.Name(activity->StandIn));
        }

        standIns?.Skipped(activity->StandIn);
        activity->Skipped = activity->StandIn;

        return true;
    }

    /// <summary>With <c>--skip-compute-chains</c>: a skipped dispatch skips the rest of its recording's compute (off by default).</summary>
    public static bool SkipComputeChains { get; set; }

    /// <summary>
    /// With <c>--empty-indirect-after-skip</c>: once a compute dispatch of a recording is skipped for a stand-in, the
    /// recording's later ExecuteIndirects run nothing (off by default). Unlike <see cref="SkipComputeChains"/> direct
    /// dispatches and draws still run: only work whose size comes from GPU-written arguments, which the skipped pass
    /// may have been meant to write (culling, argument building), is dropped.
    /// </summary>
    public static bool EmptyIndirectAfterSkip { get; set; }

    /// <summary>
    /// Whether a dispatch or ExecuteIndirect is skipped: its pipeline is a stand-in, or, with
    /// <see cref="SkipComputeChains"/>, an earlier one of the recording was. The game's compute chains (culling, argument building) prepare counters and arguments for the passes after
    /// them, and the whole 174 MB transient pool is one raw UAV, so a pass that runs after a skipped initialiser can
    /// write records anywhere in it; skipping the rest of the chain for a frame is harmless.
    /// </summary>
    [HotPath]
    private static bool ComputeChainBroken(XboxObject* self, bool producer)
    {
        var bindings = CommandListBindings.Of(self);

        if (bindings->ComputeSkipped != 0)
        {
            ShimContext.Runtime.D3D12X.Pipelines?.StandIns.SkippedAfterChain();

            return true;
        }

        if (!PipelinePending(self, draw: false))
        {
            return false;
        }

        var standIns = ShimContext.Runtime.D3D12X.Pipelines?.StandIns;

        if (RtStandIns.Enabled && standIns is not null)
        {
            RtStandIns.AtSkip(self, standIns.Name(ListActivity.Of(self)->StandIn));
        }

        if (SkipComputeChains)
        {
            bindings->ComputeSkipped = 1;
        }

        if (producer)
        {
            bindings->ProducerSkipped = 1;
        }

        return true;
    }

    [HotPath]
    private static int ResetList(XboxObject* self, nint allocator, nint pipelineState)
    {
        *CommandListBindings.Of(self) = default;
        ListMemoryWaits.Of(self)->Reset();

        ListSegments.Of(self)->Reset(ShimContext.Runtime.D3D12X.Objects.Unwrap(allocator));
        ListReplayState.Of(self)->Set = 0;
        ListReplayState.Of(self)->VertexMask = 0;

        ListIndirectScratch.Of(self)->Used = 0;
        ListIndirectScratch.Of(self)->Budget = 0;
        ListActivity.Reset(self);
        ShimContext.Runtime.D3D12X.DrawTrace?.Reset(self);

        var objects = ShimContext.Runtime.D3D12X.Objects;
        nint inner = self->Inner;
        var reset = (delegate* unmanaged<nint, nint, nint, int>)ComObject.Slot(inner, DesktopReset);
        nint pipeline = objects.Unwrap(pipelineState);
        var standIns = ShimContext.Runtime.D3D12X.Pipelines?.StandIns;

        if (standIns is not null)
        {
            pipeline = standIns.Desktop(pipeline, out _);
        }

        ShimContext.Runtime.D3D12X.Rectangles.Reset(self, pipeline);
        nint desktop = objects.Unwrap(allocator);
        int hr = reset(inner, desktop, pipeline);

        // The allocator backs another recording list, which the console allows: record on a spare (SpareAllocators).
        if (hr == HResult.InvalidArgument && desktop != 0)
        {
            var spares = ShimContext.Runtime.D3D12X.SpareAllocators;
            hr = spares.Reset(inner, desktop, pipeline, reset);

            // Refused on a fresh allocator too: the list's last Close failed, and the desktop list is dead.
            if (hr == HResult.InvalidArgument)
            {
                nint replacement = spares.Replace(inner, desktop, pipeline);

                if (replacement != 0)
                {
                    self->Inner = replacement;
                    hr = HResult.Ok;
                }
            }
        }

        return hr;
    }

    [HotPath]
    private static void BindHeaps(XboxObject* self, uint count, nint* heaps)
    {
        var bindings = CommandListBindings.Of(self);
        nint inner = self->Inner;
        ((delegate* unmanaged<nint, uint, nint*, void>)ComObject.Slot(inner, DesktopSetDescriptorHeaps))(inner, count, heaps);
        bindings->HeapsBound = 1;
        KeepHeaps(self, count, heaps);

        // The bindless root signature indexes the heaps directly: it is set again after a heap change.
        if (bindings->GraphicsRoot != 0)
        {
            SetBindlessRoot(self, false);
        }

        if (bindings->ComputeRoot != 0)
        {
            SetBindlessRoot(self, true);
        }
    }

    [HotPath]
    private static void SetRootSignature(XboxObject* self, nint rootSignature, bool compute)
    {
        var bindings = CommandListBindings.Of(self);
        var layout = Bindings.Layouts.Find(rootSignature);

        if (compute)
        {
            bindings->ComputeLayout = layout;
            bindings->ComputeRoot = 1;
        }
        else
        {
            bindings->GraphicsLayout = layout;
            bindings->GraphicsRoot = 1;
        }

        if (bindings->HeapsBound == 0)
        {
            BindDefaultHeaps(self);
        }

        SetBindlessRoot(self, compute);
    }

    /// <summary>A list that sets a root signature before any heap gets the game's main heaps (they are the only shader-visible ones).</summary>
    [HotPath]
    private static void BindDefaultHeaps(XboxObject* self)
    {
        var window = Bindings.Window;
        nint* heaps = stackalloc nint[MaxHeaps];
        uint count = 0;

        if (window.ViewHeap != 0)
        {
            heaps[count++] = window.ViewHeap;
        }

        if (window.SamplerHeap != 0)
        {
            heaps[count++] = window.SamplerHeap;
        }

        if (count != 0)
        {
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, nint*, void>)ComObject.Slot(inner, DesktopSetDescriptorHeaps))(inner, count, heaps);
            CommandListBindings.Of(self)->HeapsBound = 1;
            KeepHeaps(self, count, heaps);
        }
    }

    /// <summary>The desktop heaps the list set, for a split list (<see cref="ListReplayState"/>).</summary>
    [HotPath]
    private static void KeepHeaps(XboxObject* self, uint count, nint* heaps)
    {
        var state = ListReplayState.Of(self);
        state->Set &= ~ListReplayState.HeapsBit;

        if (count > ListReplayState.MaxHeaps || (count != 0 && heaps == null))
        {
            return;
        }

        for (uint i = 0; i < count; i++)
        {
            state->Heaps[i] = heaps[i];
        }

        state->HeapCount = count;
        state->Set |= ListReplayState.HeapsBit;
    }

    /// <summary>
    /// After a split (<see cref="ListSplits"/>): sets the list's heaps again on its new desktop list, then the bindless
    /// root signature of each bind point the list had set; the user data goes out again at the next draw or dispatch.
    /// </summary>
    public static void Rebind(XboxObject* self)
    {
        var state = ListReplayState.Of(self);
        var bindings = CommandListBindings.Of(self);

        if ((state->Set & ListReplayState.HeapsBit) != 0)
        {
            nint inner = self->Inner;
            ((delegate* unmanaged<nint, uint, long*, void>)ComObject.Slot(inner, DesktopSetDescriptorHeaps))(inner, state->HeapCount, state->Heaps);
        }

        if (bindings->GraphicsRoot != 0)
        {
            SetBindlessRoot(self, false);
        }

        if (bindings->ComputeRoot != 0)
        {
            SetBindlessRoot(self, true);
        }
    }

    /// <summary>The bindless root signature with the fallback table; the user data goes out again at the next draw.</summary>
    [HotPath]
    private static void SetBindlessRoot(XboxObject* self, bool compute)
    {
        var bindings = CommandListBindings.Of(self);
        nint root = Bindings.Root;
        nint inner = self->Inner;

        if (root == 0)
        {
            return;
        }

        int setRoot = compute ? DesktopSetComputeRootSignature : DesktopSetGraphicsRootSignature;
        int setTable = compute ? DesktopSetComputeRootDescriptorTable : DesktopSetGraphicsRootDescriptorTable;
        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(inner, setRoot))(inner, root);

        if (Bindings.Window.ViewHeap != 0)
        {
            ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(inner, setTable))(inner, FallbackTableParameter, Bindings.Window.FallbackTable);
        }

        if (compute)
        {
            bindings->ComputeDirty = 1;
        }
        else
        {
            bindings->GraphicsDirty = 1;
        }
    }

    /// <summary>
    /// Sets the bind point's user data when it changed: as root constants (<see cref="UserDataConstants"/>), or uploaded
    /// to the ring with root parameter 0 pointed at it.
    /// </summary>
    [HotPath]
    private static void Flush(XboxObject* self, bool compute)
    {
        var bindings = CommandListBindings.Of(self);
        MirrorReach(self, bindings, compute);
        var ring = Bindings.Ring;
        int root = compute ? bindings->ComputeRoot : bindings->GraphicsRoot;
        int dirty = compute ? bindings->ComputeDirty : bindings->GraphicsDirty;
        bool constants = UserDataConstants.Enabled;

        if ((ring is null && !constants) || root == 0 || dirty == 0 || Bindings.Root == 0)
        {
            return;
        }

        uint* data = compute ? bindings->Compute : bindings->Graphics;
        uint used = UserData.Used(bindings, compute);
        nint inner = self->Inner;

        if (constants)
        {
            SetUserConstants(inner, compute, data, used);
        }
        else
        {
            ulong address = ring!.Upload(data, used);

            if (NullAddressCheck.DropsRing(self, address))
            {
                return;
            }

            int slot = compute ? DesktopSetComputeRootConstantBufferView : DesktopSetGraphicsRootConstantBufferView;
            ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(inner, slot))(inner, UserDataParameter, address);
        }

        if (compute)
        {
            bindings->ComputeDirty = 0;
        }
        else
        {
            bindings->GraphicsDirty = 0;
        }
    }

    /// <summary>
    /// Before a draw or dispatch: the tables the bound pipeline's programs read past what their binds mirrored, mirrored
    /// that far (<see cref="TableReaches"/>), as the game may have copied descriptors there since.
    /// </summary>
    [HotPath]
    private static void MirrorReach(XboxObject* self, CommandListBindings* bindings, bool compute)
    {
        nint pipeline = RectangleListState.Of(self)->Pipeline;
        var entries = TableReaches.Of(pipeline);

        if (entries.Length == 0)
        {
            return;
        }

        uint* data = compute ? bindings->Compute : bindings->Graphics;

        if (DepthUavs.Any)
        {
            DepthUavs.CheckFirstUse(pipeline, data, ShimContext.Runtime.D3D12X.Bindings.Window.Data);
        }

        if (DepthTwins.Any)
        {
            DepthTwins.MarkWriters(pipeline, data, ShimContext.Runtime.D3D12X.Bindings.Window.Data, entries);
        }

        var heaps = ShimContext.Runtime.D3D12X.DescriptorHeaps;

        foreach (var entry in entries)
        {
            if (entry.Sgpr < CommandListBindings.UserDataDwords)
            {
                heaps.MirrorTable(data[entry.Sgpr], entry.Records);
            }
        }
    }

    /// <summary>
    /// The used dwords as root constants. Past <see cref="BindlessRootSignature.UserConstants"/> (a layout the shim could
    /// not read, which uploads all 128) is nothing a program reads: RDNA preloads no user SGPR past s31.
    /// </summary>
    [HotPath]
    private static void SetUserConstants(nint inner, bool compute, uint* data, uint used)
    {
        int slot = compute ? DesktopSetComputeRoot32BitConstants : DesktopSetGraphicsRoot32BitConstants;

        if (used > BindlessRootSignature.UserConstants)
        {
            used = BindlessRootSignature.UserConstants;
        }

        ((delegate* unmanaged<nint, uint, uint, uint*, uint, void>)ComObject.Slot(inner, slot))(inner, UserDataParameter, used, data, 0);
    }

    [HotPath]
    private static void WriteConstants(CommandListBindings* bindings, bool compute, uint parameter, uint count, uint* values, uint offset)
    {
        for (uint i = 0; i < count; i++)
        {
            UserData.Write(bindings, compute, parameter, offset + i, values[i]);
        }
    }

    /// <summary>The window pointer of a table, with as many records mirrored as the bound root signature's table holds.</summary>
    [HotPath]
    private static uint TableAddress(CommandListBindings* bindings, bool compute, uint parameter, ulong handle)
    {
        RootLayout* layout = compute ? bindings->ComputeLayout : bindings->GraphicsLayout;
        uint extent = DescriptorWindow.MaxTableRecords;

        if (layout != null && parameter < (uint)layout->Count)
        {
            extent = layout->Extents[parameter];
        }

        return ShimContext.Runtime.D3D12X.DescriptorHeaps.TableAddress(handle, DescriptorWindow.MirroredRecords(extent));
    }
}
