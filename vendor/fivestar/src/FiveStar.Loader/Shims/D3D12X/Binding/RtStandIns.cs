// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Present;
using FiveStar.Loader.Shims.D3D12X.States;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Stand-in outputs for the ray-tracing passes held without <c>--raytracing</c> (default on; <c>--no-rt-stand-ins</c>
/// turns them off): when such a pass's dispatch is skipped, the texture UAVs of its compute tables get a constant
/// instead of what a frame ago or nothing left there (<see cref="UavFill"/>). Ray-traced shadow masks read fully lit,
/// planar reflections black; the screen-probe light, whose rays are held, is overwritten after
/// CS_ScreenProbes_TemporalResolve runs (or at its skip) with a modest grey, as VIctory's ambient stand-in does
/// (ZJOU-VICTORY 1fc58f3 after Belobug 4f45ab8; vitutu1 c3faf2a adds shadow-mask and exposure stand-ins; facts only).
/// Raw-buffer writers (the screen-probe rays, the deferred gather) are left alone.
/// </summary>
internal static unsafe class RtStandIns
{
    /// <summary>ID3D12Device::GetDescriptorHandleIncrementSize and the CBV_SRV_UAV heap type (d3d12.h).</summary>
    private const int IncrementSlot = 15;
    private const int ViewHeapType = 0;

    private static readonly (string Name, FillValue Value, bool AfterRun)[] Rules =
    [
        ("shadowmask~CS_TraceShadowRays", new FillValue(1, 1, 1, uint.MaxValue), false),
        ("rtdirectionalshadows~", new FillValue(1, 1, 1, uint.MaxValue), false),
        ("CS_PPR_Raytrace", new FillValue(0, 0, 0, 0), false),
        ("CS_ScreenProbes_TemporalResolve", FillValue.Shared(AmbientLevel), true),
    ];

    private static readonly ConcurrentDictionary<nint, int> RuleOfPipeline = new();
    private static long _filled;

    /// <summary>The grey of the screen-probe light stand-in (VIctory's 0.6).</summary>
    public const float AmbientLevel = 0.6f;

    /// <summary>Whether held passes get stand-in outputs (default on; off with <c>--no-rt-stand-ins</c>).</summary>
    public static bool Enabled { get; set; } = true;

    /// <summary>UAVs filled so far.</summary>
    public static long Filled => Interlocked.Read(ref _filled);

    /// <summary>The rule for a pipeline name, or -1.</summary>
    public static int RuleOf(string name)
    {
        return Array.FindIndex(Rules, rule => name.Contains(rule.Name, StringComparison.Ordinal));
    }

    /// <summary>A dispatch of a held pipeline named <paramref name="name"/> was skipped: its outputs get the rule's value.</summary>
    public static void AtSkip(XboxObject* list, string name)
    {
        int rule = RuleOf(name);

        if (rule >= 0)
        {
            Fill(list, Rules[rule].Value);
        }
    }

    /// <summary>After a dispatch that ran: a pass whose rule writes after it (the screen-probe light) gets its outputs overwritten.</summary>
    [HotPath]
    public static void AfterDispatch(XboxObject* list, nint pipeline)
    {
        if (!RuleOfPipeline.TryGetValue(pipeline, out int rule))
        {
            rule = RuleOf(QueueProgress.PipelineName(pipeline));
            RuleOfPipeline[pipeline] = rule;
        }

        if (rule >= 0 && Rules[rule].AfterRun)
        {
            Fill(list, Rules[rule].Value);
        }
    }

    /// <summary>Writes <paramref name="value"/> over the texture UAVs of the list's compute tables, then sets the list's compute state again.</summary>
    private static void Fill(XboxObject* list, FillValue value)
    {
        var runtime = ShimContext.Runtime.D3D12X;
        var bindings = CommandListBindings.Of(list);
        var layout = bindings->ComputeLayout;
        var window = runtime.Bindings.Window;

        if (layout == null || layout->UavRanges == 0 || window.Data == null || window.ViewHeap == 0)
        {
            return;
        }

        nint inner = list->Inner;
        int filled = 0;

        for (int range = 0; range < layout->UavRanges; range++)
        {
            uint pointer = bindings->Compute[UserData.ComputeBase + layout->Offsets[layout->UavParameters[range]]];
            uint end = (uint)(layout->UavStarts[range] + layout->UavCounts[range]);
            runtime.DescriptorHeaps.MirrorTable(pointer, end);
            filled += FillRecords(inner, window, pointer, layout->UavStarts[range], end, value, filled == 0);
        }

        if (filled == 0)
        {
            return;
        }

        Barrier(inner);
        Interlocked.Add(ref _filled, filled);
        CommandListBindingMethods.RestoreCompute(list);
    }

    /// <summary>Fills the texture UAVs of records <paramref name="first"/> to <paramref name="end"/> of the table at window offset <paramref name="pointer"/>; how many.</summary>
    private static int FillRecords(nint list, DescriptorWindow window, uint pointer, uint first, uint end, FillValue value, bool barrierFirst)
    {
        nint device = ShimContext.Runtime.D3D12X.Objects.DesktopDevice;
        nint cpuStart = DesktopCalls.HeapCpuStart(window.ViewHeap);
        ulong gpuStart = DesktopCalls.HeapGpuStart(window.ViewHeap);
        uint increment = ((delegate* unmanaged<nint, int, uint>)ComObject.Slot(device, IncrementSlot))(device, ViewHeapType);
        int filled = 0;

        for (uint record = first; record < end; record++)
        {
            ulong offset = pointer + ((ulong)record * DescriptorWindow.ViewRecordBytes);

            if (offset + DescriptorWindow.ViewRecordBytes > DescriptorWindow.NullTableBase)
            {
                break;
            }

            uint index = ((uint*)(window.Data + offset))[DescriptorWindow.MirroredDesktopDword];

            if (!UavViews.TryFind(cpuStart + (nint)((ulong)index * increment), out var view))
            {
                continue;
            }

            if (barrierFirst && filled == 0)
            {
                Barrier(list);
            }

            if (UavFill.Record(device, list, gpuStart, index, view, value))
            {
                filled++;
            }
        }

        return filled;
    }

    /// <summary>A UAV barrier on every resource: the fill after the pass's writes, the readers after the fill.</summary>
    private static void Barrier(nint list)
    {
        var barrier = new TransitionBarrier { Type = TransitionBarrier.UavType, Resource = 0 };
        BarrierWriter.Write(list, &barrier);
    }
}
