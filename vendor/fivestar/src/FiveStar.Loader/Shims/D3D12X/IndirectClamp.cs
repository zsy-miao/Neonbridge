// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using FiveStar.Loader.Shims.D3D12X.Present;
using FiveStar.Loader.Shims.D3D12X.States;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Bounds indirect arguments before the GPU runs them. The game's own compute passes write them; a pass that is
/// skipped (a stand-in pipeline) or translated wrongly leaves whatever the memory held, and a draw of four billion
/// vertices or a dispatch of 0xFFFFFFFF groups runs until the driver resets the device. Before an ExecuteIndirect with a
/// one-argument signature (all of the game's), a compute program of ours copies the records into the list's scratch
/// buffer (<see cref="ListIndirectScratch"/>) and clamps them, and the list executes the copy; the game's buffers are
/// not touched. Bounds: 2^24 vertices and 2^26 vertex instances per draw, 65,535 groups per dimension and 2^20 per
/// dispatch (more runs nothing), mesh dispatches one-dimensional with a first dimension over 65,535 run empty (a record
/// the game did not fill). A recording also has a budget of 2^27 work items (a vertex instance 1, a compute group
/// 64, a mesh group 128): records past it run empty. Each record alone stays within the bounds, but a list of
/// thousands of stale records (culling passes skipped for stand-ins) passed the driver's 5 s timeout (TDR at about
/// 300 s into a boot). The same mechanism and budget as victory-gta6's argument clamp (`indirect.rs`, `bound_ahead`).
/// </summary>
internal static unsafe class IndirectClamp
{
    /// <summary>D3D12_INDIRECT_ARGUMENT_TYPE_DRAW, _DRAW_INDEXED, _DISPATCH and _DISPATCH_MESH (d3d12.h).</summary>
    public const int DrawKind = 0;
    public const int DrawIndexedKind = 1;
    public const int DispatchKind = 2;
    public const int MeshKind = 10;

    /// <summary>Root parameters of the program: four constants, the arguments, the count, the copy.</summary>
    private const uint ConstantsParameter = 0;
    private const uint ArgumentsParameter = 1;
    private const uint CountParameter = 2;
    private const uint ClampedParameter = 3;
    private const uint BudgetParameter = 4;
    private const uint Constants = 4;
    private const uint ZeroKind = 255;
    private const uint GroupThreads = 64;

    /// <summary>ID3D12Device::CreateComputePipelineState and CreateRootSignature (d3d12.h).</summary>
    private const int CreateComputePipelineSlot = 11;
    private const int CreateRootSignatureSlot = 16;

    /// <summary>ID3D12GraphicsCommandList: Dispatch, SetPipelineState, SetComputeRootSignature, SetComputeRoot32BitConstants, SetComputeRootShaderResourceView, SetComputeRootUnorderedAccessView.</summary>
    private const int DispatchSlot = 14;
    private const int SetPipelineSlot = 25;
    private const int SetRootSignatureSlot = 29;
    private const int SetConstantsSlot = 35;
    private const int SetShaderResourceSlot = 39;
    private const int SetUnorderedAccessSlot = 41;

    /// <summary>Scratch pieces start on 256 bytes; the first buffer of a list holds 64 KiB.</summary>
    private const ulong Alignment = 256;
    private const ulong FirstScratchBytes = 64 << 10;
    private const ulong Growth = 2;

    /// <summary>D3D12_HEAP_TYPE_DEFAULT.</summary>
    private const int DefaultHeap = 1;

    // Work is counted as victory-gta6 does: a vertex instance is 1, a compute group 64, a mesh group 128. A record
    // whose work does not fit what is left of the list's budget runs empty and charges nothing; the charge is a
    // compare-exchange so the count never wraps. Kind 255 zeroes the list's counter.
    private const string Source = """
        #define RS "RootConstants(num32BitConstants=4, b0), SRV(t0), SRV(t1), UAV(u0), UAV(u1)"
        cbuffer Clamp : register(b0) { uint kind; uint stride; uint records; uint counted; };
        ByteAddressBuffer arguments : register(t0);
        ByteAddressBuffer countBuffer : register(t1);
        RWByteAddressBuffer clamped : register(u0);
        RWByteAddressBuffer budget : register(u1);
        static const uint MaxVertices = 1u << 24;
        static const uint MaxVertexInstances = 1u << 26;
        static const uint MaxGroupsPerDimension = 65535u;
        static const uint MaxGroups = 1u << 20;
        static const uint ListBudget = 1u << 27;
        static const uint ZeroKind = 255u;
        static const uint Tries = 64u;
        bool Charge(uint work) {
            if (work == 0u) return true;
            if (work > ListBudget) return false;
            uint seen;
            budget.InterlockedAdd(0, 0u, seen);
            [loop] for (uint i = 0; i < Tries; i++) {
                if (seen > ListBudget - work) return false;
                uint original;
                budget.InterlockedCompareExchange(0, seen, seen + work, original);
                if (original == seen) return true;
                seen = original;
            }
            return false;
        }
        [RootSignature(RS)]
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
            if (kind == ZeroKind) { if (id.x == 0u) budget.Store(0, 0u); return; }
            if (id.x >= records) return;
            uint at = id.x * stride;
            for (uint word = 0; word < stride; word += 4) clamped.Store(at + word, arguments.Load(at + word));
            uint live = records;
            if (counted != 0u) live = min(live, countBuffer.Load(0));
            if (id.x >= live) return;
            if (kind <= 1u) {
                uint2 asked = arguments.Load2(at);
                uint vertices = min(asked.x, MaxVertices);
                uint instances = min(asked.y, MaxVertexInstances / max(vertices, 1u));
                if (!Charge(vertices * instances)) instances = 0u;
                clamped.Store2(at, uint2(vertices, instances));
            } else if (kind == 2u) {
                uint3 groups = min(arguments.Load3(at), uint3(MaxGroupsPerDimension, MaxGroupsPerDimension, MaxGroupsPerDimension));
                if (groups.z != 0u && groups.x * groups.y > MaxGroups / groups.z) groups = uint3(0u, 0u, 0u);
                if (!Charge(groups.x * groups.y * groups.z * 64u)) groups = uint3(0u, 0u, 0u);
                clamped.Store3(at, groups);
            } else {
                uint3 groups = arguments.Load3(at);
                if (groups.x > MaxGroupsPerDimension) groups = uint3(0u, 0u, 0u);
                groups = uint3(groups.x, min(groups.y, 1u), min(groups.z, 1u));
                if (!Charge(groups.x * groups.y * groups.z * 128u)) groups = uint3(0u, 0u, 0u);
                clamped.Store3(at, groups);
            }
        }
        """;

    private static readonly object Gate = new();
    private static readonly Guid RootSignatureIid = new("c54a6b66-72df-4ee8-8be5-a946a1429214");
    private static readonly Guid PipelineIid = new("765a30f3-f624-4c6f-a828-ace948622445");
    private static nint _device;
    private static nint _root;
    private static nint _pipeline;
    private static bool _failed;
    private static long _clamped;

    /// <summary>ExecuteIndirect calls whose arguments were clamped.</summary>
    public static long Clamped => Interlocked.Read(ref _clamped);

    /// <summary>
    /// Records the clamp of <paramref name="maxCount"/> records of signature <paramref name="signature"/> from desktop
    /// buffer <paramref name="arguments"/> at <paramref name="argumentOffset"/> (bounded by the count at
    /// <paramref name="count"/>, when given) into the list's scratch; returns the scratch and its offset to execute
    /// instead, or false when the arguments run as they are. Replaces the list's compute root signature and pipeline:
    /// the caller sets its own again.
    /// </summary>
    public static bool Record(XboxObject* self, nint signature, uint maxCount, nint arguments, ulong argumentOffset, nint count, ulong countOffset, out nint scratch, out ulong scratchOffset)
    {
        scratch = 0;
        scratchOffset = 0;
        var (kind, stride) = CommandSignatures.Clampable(signature);
        nint list = self->Inner;

        if (kind < 0 || arguments == 0 || maxCount == 0 || !Ready())
        {
            return false;
        }

        ulong bytes = (ulong)stride * maxCount;

        // Each piece leaves room in front for the list's budget counter, used when the list has none in this buffer.
        if (bytes > uint.MaxValue || !Take(self, bytes + Alignment, out scratch, out scratchOffset))
        {
            return false;
        }

        ulong budget = BudgetAddress(self, scratch, scratchOffset, out bool zero);
        scratchOffset += Alignment;
        ulong destination = DesktopCalls.GpuAddress(scratch) + scratchOffset;
        uint counted = 0;
        ulong countAddress = DesktopCalls.GpuAddress(arguments) + argumentOffset;

        if (count != 0)
        {
            counted = 1;
            countAddress = DesktopCalls.GpuAddress(count) + countOffset;
        }

        if (NullAddressCheck.Enabled && (NullAddressCheck.Suspect(destination) || NullAddressCheck.Suspect(budget) || NullAddressCheck.Suspect(countAddress)))
        {
            return !NullAddressCheck.Drop(NullAddressKind.RootDescriptor, self, $"argument clamp views: clamped 0x{destination:x}, budget 0x{budget:x}, count 0x{countAddress:x}");
        }

        BarrierWriter.Transition(list, scratch, ResourceStates.Common, ResourceStates.UnorderedAccess);
        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetRootSignatureSlot))(list, _root);
        ((delegate* unmanaged<nint, nint, void>)ComObject.Slot(list, SetPipelineSlot))(list, _pipeline);

        ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(list, SetShaderResourceSlot))(list, ArgumentsParameter, DesktopCalls.GpuAddress(arguments) + argumentOffset);
        ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(list, SetShaderResourceSlot))(list, CountParameter, countAddress);
        ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(list, SetUnorderedAccessSlot))(list, ClampedParameter, destination);
        ((delegate* unmanaged<nint, uint, ulong, void>)ComObject.Slot(list, SetUnorderedAccessSlot))(list, BudgetParameter, budget);

        if (zero)
        {
            ZeroBudget(list, scratch);
        }

        uint* constants = stackalloc uint[] { (uint)kind, stride, maxCount, counted };
        ((delegate* unmanaged<nint, uint, uint, uint*, uint, void>)ComObject.Slot(list, SetConstantsSlot))(list, ConstantsParameter, Constants, constants, 0);
        ((delegate* unmanaged<nint, uint, uint, uint, void>)ComObject.Slot(list, DispatchSlot))(list, (maxCount + GroupThreads - 1) / GroupThreads, 1, 1);
        BarrierWriter.Transition(list, scratch, ResourceStates.UnorderedAccess, IndirectArgumentState);
        Interlocked.Increment(ref _clamped);

        return true;
    }

    /// <summary>
    /// The address of the recording's budget counter; a recording without one in this buffer takes the room in front of
    /// <paramref name="piece"/> and must zero it (<paramref name="zero"/>).
    /// </summary>
    private static ulong BudgetAddress(XboxObject* self, nint scratch, ulong piece, out bool zero)
    {
        var state = ListIndirectScratch.Of(self);
        zero = state->Budget == 0;

        if (zero)
        {
            state->Budget = piece + 1;
        }

        return DesktopCalls.GpuAddress(scratch) + state->Budget - 1;
    }

    /// <summary>The list's first clamp in this buffer zeroes the budget counter, then waits for the store (UAV barrier).</summary>
    private static void ZeroBudget(nint list, nint scratch)
    {
        uint* zero = stackalloc uint[] { ZeroKind, 0, 0, 0 };
        ((delegate* unmanaged<nint, uint, uint, uint*, uint, void>)ComObject.Slot(list, SetConstantsSlot))(list, ConstantsParameter, Constants, zero, 0);
        ((delegate* unmanaged<nint, uint, uint, uint, void>)ComObject.Slot(list, DispatchSlot))(list, 1, 1, 1);

        var barrier = new TransitionBarrier { Type = TransitionBarrier.UavType, Resource = scratch };
        BarrierWriter.Write(list, &barrier);
    }

    /// <summary>After the ExecuteIndirect that read the scratch: back to COMMON, where the next clamp expects it.</summary>
    public static void Done(XboxObject* self, nint scratch)
    {
        BarrierWriter.Transition(self->Inner, scratch, IndirectArgumentState, ResourceStates.Common);
    }

    /// <summary>D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT.</summary>
    private const int IndirectArgumentState = 0x200;

    /// <summary>The next aligned piece of <paramref name="bytes"/> in the list's scratch, growing it when full.</summary>
    private static bool Take(XboxObject* self, ulong bytes, out nint buffer, out ulong offset)
    {
        var scratch = ListIndirectScratch.Of(self);
        offset = (scratch->Used + Alignment - 1) & ~(Alignment - 1);

        if (scratch->Buffer == 0 || offset + bytes > scratch->Bytes)
        {
            ulong size = Math.Max(FirstScratchBytes, scratch->Bytes * Growth);

            while (size < bytes)
            {
                size *= Growth;
            }

            var properties = new HeapProperties { Type = DefaultHeap };
            nint grown = DesktopCalls.CreateBuffer(_device, properties, size, DesktopCalls.UnorderedAccessFlag, ResourceStates.Common);

            if (grown == 0)
            {
                buffer = 0;

                return false;
            }

            // The outgrown buffer stays alive: commands already recorded may read it. The budget starts over in the new one.
            scratch->Buffer = grown;
            scratch->Bytes = size;
            scratch->Budget = 0;
            offset = 0;
        }

        scratch->Used = offset + bytes;
        buffer = scratch->Buffer;

        return true;
    }

    /// <summary>The program and its root signature, made on first use.</summary>
    private static bool Ready()
    {
        if (_pipeline != 0)
        {
            return true;
        }

        lock (Gate)
        {
            if (_pipeline == 0 && !_failed)
            {
                _failed = !Make();
            }

            return _pipeline != 0;
        }
    }

    private static bool Make()
    {
        nint device = ShimContext.Runtime.D3D12X.Objects.DesktopDevice;
        nint code = Compile();

        if (device == 0 || code == 0)
        {
            return false;
        }

        byte* bytes = ((delegate* unmanaged<nint, byte*>)ComObject.Slot(code, IndirectClampNative.BlobPointerSlot))(code);
        nuint size = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(code, IndirectClampNative.BlobSizeSlot))(code);
        Guid rootIid = RootSignatureIid;
        nint root = 0;
        int hr = ((delegate* unmanaged<nint, uint, byte*, nuint, Guid*, nint*, int>)ComObject.Slot(device, CreateRootSignatureSlot))(device, 0, bytes, size, &rootIid, &root);

        if (!HResult.Failed(hr))
        {
            var description = new ComputePipelineDescription { RootSignature = root, Code = bytes, CodeSize = size };
            Guid pipelineIid = PipelineIid;
            nint pipeline = 0;
            hr = ((delegate* unmanaged<nint, ComputePipelineDescription*, Guid*, nint*, int>)ComObject.Slot(device, CreateComputePipelineSlot))(device, &description, &pipelineIid, &pipeline);
            _pipeline = pipeline;
        }

        ComObject.Release(code);

        if (HResult.Failed(hr))
        {
            ShimContext.Runtime.Log.Warn($"[d3d12_xs] indirect-argument clamp: no program ({HResult.Format(hr)}); arguments run unclamped");

            return false;
        }

        _device = device;
        _root = root;

        return true;
    }

    /// <summary>The program compiled by d3dcompiler_47 (cs_5_1, its root signature inside); 0 when that fails.</summary>
    private static nint Compile()
    {
        byte[] source = Encoding.ASCII.GetBytes(Source);
        byte[] entry = Encoding.ASCII.GetBytes("main\0");
        byte[] target = Encoding.ASCII.GetBytes("cs_5_1\0");
        byte[] name = Encoding.ASCII.GetBytes("indirect_clamp\0");

        nint code = 0;
        nint errors = 0;
        int hr;

        fixed (byte* text = source, main = entry, profile = target, file = name)
        {
            hr = IndirectClampNative.D3DCompile(text, (nuint)source.Length, file, 0, 0, main, profile, 0, 0, &code, &errors);
        }

        if (errors != 0)
        {
            byte* message = ((delegate* unmanaged<nint, byte*>)ComObject.Slot(errors, IndirectClampNative.BlobPointerSlot))(errors);
            nuint length = ((delegate* unmanaged<nint, nuint>)ComObject.Slot(errors, IndirectClampNative.BlobSizeSlot))(errors);
            ShimContext.Runtime.Log.Warn($"[d3d12_xs] indirect-argument clamp: {Encoding.ASCII.GetString(message, (int)length).Trim()}");
            ComObject.Release(errors);
        }

        if (HResult.Failed(hr))
        {
            return 0;
        }

        return code;
    }

    /// <summary>D3D12_COMPUTE_PIPELINE_STATE_DESC (d3d12.h).</summary>
    private struct ComputePipelineDescription
    {
        public nint RootSignature;
        public byte* Code;
        public nuint CodeSize;
        public uint NodeMask;
        public nint CachedBlob;
        public nuint CachedBlobSize;
        public uint Flags;
    }
}
