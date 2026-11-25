// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Collections.Concurrent;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// Which passes write a depth texture through a UAV, which the desktop refuses (the view becomes null and the writes are
/// lost; docs/d3d12x-shim.md, resource descs). Each depth resource that gets a UAV is logged once with the heap index its
/// view went to; a pipeline whose tables (<see cref="TableReaches"/>) hold one of those indices is named at its first
/// use; and the depth test and stencil ops of each tiled-lighting pipeline are logged when it is built. Groundwork for
/// a colour twin that takes those writes.
/// </summary>
internal static unsafe class DepthUavs
{
    /// <summary>The program name part of the tiled-lighting draws whose depth state is logged.</summary>
    public const string LightingPrograms = "TL_PS";

    private const uint RecordBytes = DescriptorWindow.ViewRecordBytes;
    private const int BaseHighDword = 1;
    private const int SizeDword = 2;
    private const int TypeDword = 3;

    // D3D12_DEPTH_STENCIL_DESC1: DepthEnable, DepthWriteMask, DepthFunc, StencilEnable, masks, FrontFace {fail, depth
    // fail, pass, func}, BackFace (d3d12.h).
    private const int DepthWriteField = 0x04;
    private const int StencilMasksField = 0x10;
    private const int FrontFaceField = 0x14;
    private const int BackFaceField = 0x24;
    private const int FaceInts = 4;

    private static readonly ConcurrentDictionary<uint, string> Indices = new();
    private static readonly ConcurrentDictionary<ulong, byte> Resources = new();
    private static readonly ConcurrentDictionary<nint, byte> Checked = new();
    private static readonly ConcurrentDictionary<string, byte> Lighting = new(StringComparer.Ordinal);

    /// <summary>Whether a depth resource got a UAV: pipelines are then checked at first use.</summary>
    public static bool Any => !Indices.IsEmpty;

    /// <summary>A UAV of a depth texture (<paramref name="what"/>) went to heap index <paramref name="index"/>; logs the resource once.</summary>
    public static void Note(ulong resource, uint index, string what)
    {
        Indices[index] = what;

        if (Resources.TryAdd(resource, 0))
        {
            ShimContext.Log?.Warn($"[depth uav] {what}: its UAV is a null view (heap index {index}); its writes are lost");
        }
    }

    /// <summary>The heap index a record names, when it holds one (a view record without console fields); null otherwise.</summary>
    public static uint? HeapIndex(uint* record)
    {
        if (record[BaseHighDword] != 0 || record[SizeDword] != 0 || record[TypeDword] != 0)
        {
            return null;
        }

        return record[0];
    }

    /// <summary>
    /// At a draw or dispatch: names <paramref name="pipeline"/> once if a table it reads (<paramref name="userData"/>
    /// pointers and <see cref="TableReaches"/>) holds a depth UAV's heap index.
    /// </summary>
    public static void CheckFirstUse(nint pipeline, uint* userData, byte* window)
    {
        if (window == null || !Checked.TryAdd(pipeline, 0))
        {
            return;
        }

        foreach (var entry in TableReaches.Of(pipeline))
        {
            if (entry.Sgpr >= CommandListBindings.UserDataDwords)
            {
                continue;
            }

            uint table = userData[entry.Sgpr];

            for (uint i = 0; i < entry.Records && table + ((i + 1) * RecordBytes) <= DescriptorWindow.RecordsEnd; i++)
            {
                uint? index = HeapIndex((uint*)(window + table + (i * RecordBytes)));

                if (index.HasValue && Indices.TryGetValue(index.Value, out var what))
                {
                    ShimContext.Log?.Warn($"[depth uav] {QueueProgress.PipelineName(pipeline)} binds the depth UAV of {what} (user SGPR {entry.Sgpr}, record {i})");
                }
            }
        }
    }

    /// <summary>Logs the depth test and stencil ops of a tiled-lighting pipeline once, as the desktop gets them.</summary>
    public static void NoteDepthState(IReadOnlyList<TranslatedProgram> programs, byte[] depthStencil)
    {
        string names = string.Join(" + ", programs.Select(program => program.Name));

        if (!names.Contains(LightingPrograms, StringComparison.Ordinal) || !Lighting.TryAdd(names, 0))
        {
            return;
        }

        ShimContext.Log?.Info($"  [depth uav] {names}: {DescribeDepthState(depthStencil)}");
    }

    /// <summary>A D3D12_DEPTH_STENCIL_DESC1 as text: depth enable, write, func; stencil enable, masks, front and back ops.</summary>
    public static string DescribeDepthState(byte[] desc)
    {
        int Read(int at) => BinaryPrimitives.ReadInt32LittleEndian(desc.AsSpan(at));

        string Face(int at) => string.Join(' ', Enumerable.Range(0, FaceInts).Select(i => Read(at + (i * sizeof(int)))));

        return $"depth {Read(DesktopStateBytes.DepthEnableField)} write {Read(DepthWriteField)} func {Read(DesktopStateBytes.DepthFuncField)}; "
            + $"stencil {Read(DesktopStateBytes.StencilEnableField)} read 0x{desc[StencilMasksField]:x2} write 0x{desc[StencilMasksField + 1]:x2} "
            + $"front (fail depth-fail pass func) {Face(FrontFaceField)} back {Face(BackFaceField)}";
    }
}
