// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// With <c>--watch-transient</c> and <c>--d3d12-breadcrumbs</c>: each marked draw, dispatch and ExecuteIndirect has
/// its user data searched for addresses in the transient pool. Every dword pair whose 48-bit value (a root
/// descriptor's address, or a V#'s base: dword 0 and the low 16 bits of dword 1) reaches into the pool is kept in a
/// ring with the four dwords from there (a V#'s stride in bits 16 to 29 of dword 1, its record count in dword 2), the
/// operation's recording and index and its pipeline. Buffer UAVs the operation's descriptor tables name are added by
/// <see cref="TransientPoolViews"/>. A report names the operations whose ranges reach a block.
/// </summary>
internal static unsafe class TransientPoolHits
{
    private const int Ring = 1 << 16;
    private const int BaseHighBits = 0xFFFF;
    private const int StrideShift = 16;
    private const uint StrideMask = 0x3FFF;
    private const int HighShift = 32;

    /// <summary>A V#'s dwords after the base pair: the record count, then the format and swizzle word.</summary>
    private const uint RecordsDword = 2;
    private const uint LastDword = 3;

    /// <summary>A range longer than this is taken as a pair that only looks like a V#; its base alone is kept.</summary>
    private const ulong MaxRange = 1UL << 28;

    private static readonly Hit[] Hits = new Hit[Ring];
    private static long _count;
    private static ulong _poolStart;
    private static ulong _poolEnd;

    /// <summary>One pool address in an operation's user data.</summary>
    private struct Hit
    {
        public uint Serial;
        public int Index;
        public ulong Base;
        public ulong Size;
        public nint Pipeline;
        public bool Compute;
        public uint Dword;
        public uint D0;
        public uint D1;
        public uint D2;
        public uint D3;

        /// <summary>The descriptor's position in the table the user dword points at; -1 for a user-data pair.</summary>
        public int TableSlot;
    }

    /// <summary>The pool's range, once the game has set it up; searching starts then.</summary>
    public static void Pool(ulong start, ulong end)
    {
        _poolStart = start;
        Volatile.Write(ref _poolEnd, end);
    }

    /// <summary>Before marked operation <paramref name="index"/> of recording <paramref name="serial"/>.</summary>
    [HotPath]
    public static void Note(XboxObject* list, uint serial, int index, MarkedKind kind, nint pipeline)
    {
        if (Volatile.Read(ref _poolEnd) == 0 || kind == MarkedKind.Command)
        {
            return;
        }

        var bindings = CommandListBindings.Of(list);

        if (kind != MarkedKind.Draw)
        {
            Search(bindings->Compute, serial, index, pipeline, true);
            TransientPoolViews.Search(bindings->Compute, serial, index, pipeline, true);
        }

        if (kind != MarkedKind.Dispatch)
        {
            Search(bindings->Graphics, serial, index, pipeline, false);
            TransientPoolViews.Search(bindings->Graphics, serial, index, pipeline, false);
        }
    }

    /// <summary>The distinct pipelines of the kept hits of recording <paramref name="serial"/>, with their operation indices.</summary>
    public static void DescribeRecording(StringBuilder text, uint serial, Func<nint, string> pipelineName)
    {
        long count = Interlocked.Read(ref _count);
        var seen = new HashSet<nint>();

        for (long i = count - 1; i >= 0 && i >= count - Ring; i--)
        {
            var hit = Hits[i % Ring];

            if (hit.Serial == serial && seen.Add(hit.Pipeline))
            {
                text.Append(CultureInfo.InvariantCulture, $" operation {hit.Index} {pipelineName(hit.Pipeline)};");
            }
        }
    }

    /// <summary>Whether [<paramref name="start"/>, +<paramref name="bytes"/>) overlaps the pool.</summary>
    [HotPath]
    public static bool InPool(ulong start, ulong bytes)
    {
        ulong end = start + bytes;

        if (bytes == 0)
        {
            end = start + 1;
        }

        return start < Volatile.Read(ref _poolEnd) && end > _poolStart;
    }

    /// <summary>A buffer UAV in a descriptor table (<see cref="TransientPoolViews"/>): its game range and the window record's words.</summary>
    [HotPath]
    public static void AddTable(uint serial, int index, nint pipeline, bool compute, uint dword, int tableSlot, ulong start, ulong bytes, uint* record)
    {
        long slot = Interlocked.Increment(ref _count) - 1;
        Hits[slot % Ring] = new Hit
        {
            Serial = serial,
            Index = index,
            Base = start,
            Size = bytes,
            Pipeline = pipeline,
            Compute = compute,
            Dword = dword,
            D0 = record[0],
            D1 = record[1],
            D2 = record[RecordsDword],
            D3 = record[LastDword],
            TableSlot = tableSlot,
        };
        TransientPoolLifetime.Touch(serial, start, bytes);
    }

    [HotPath]
    private static void Search(uint* data, uint serial, int index, nint pipeline, bool compute)
    {
        for (uint i = 0; i + 1 < CommandListBindings.UserDataDwords; i++)
        {
            ulong start = data[i] | ((ulong)(data[i + 1] & BaseHighBits) << HighShift);

            if (start >= _poolEnd || start + MaxRange <= _poolStart)
            {
                continue;
            }

            ulong size = Size(data, i);
            ulong end = start + size;

            if (size == 0)
            {
                end = start + 1;
            }

            if (end <= _poolStart)
            {
                continue;
            }

            long slot = Interlocked.Increment(ref _count) - 1;
            Hits[slot % Ring] = new Hit
            {
                Serial = serial,
                Index = index,
                Base = start,
                Size = size,
                Pipeline = pipeline,
                Compute = compute,
                Dword = i,
                D0 = data[i],
                D1 = data[i + 1],
                D2 = Dword(data, i + RecordsDword),
                D3 = Dword(data, i + LastDword),
                TableSlot = -1,
            };
            TransientPoolLifetime.Touch(serial, start, size);
        }
    }

    /// <summary>A V#'s range (stride times records, or records bytes for stride 0); 0 when it would be too long.</summary>
    private static ulong Size(uint* data, uint i)
    {
        ulong stride = (data[i + 1] >> StrideShift) & StrideMask;
        ulong records = Dword(data, i + RecordsDword);
        ulong size = records * Math.Max(stride, 1);

        if (size > MaxRange)
        {
            return 0;
        }

        return size;
    }

    private static uint Dword(uint* data, uint i)
    {
        if (i < CommandListBindings.UserDataDwords)
        {
            return data[i];
        }

        return 0;
    }

    /// <summary>
    /// Appends the newest kept hits whose range reaches [<paramref name="block"/>, +<paramref name="size"/>), newest
    /// first; <paramref name="operations"/>, when given, keeps only hits of those (serial, index) pairs.
    /// </summary>
    public static void Describe(StringBuilder text, ulong block, ulong size, HashSet<(uint, int)>? operations, int limit, Func<nint, string> pipelineName)
    {
        long count = Interlocked.Read(ref _count);
        int printed = 0;

        for (long i = count - 1; i >= 0 && i >= count - Ring && printed < limit; i--)
        {
            var hit = Hits[i % Ring];
            ulong end = hit.Base + Math.Max(hit.Size, 1);

            if (hit.Base >= block + size || end <= block || (operations is not null && !operations.Contains((hit.Serial, hit.Index))))
            {
                continue;
            }

            string stage = hit.Compute ? "compute" : "graphics";
            string where = $"user dword {hit.Dword}";

            if (hit.TableSlot >= 0)
            {
                where = $"UAV {hit.TableSlot} of the table at user dword {hit.Dword}, desktop index {hit.D0}";
            }

            long offset = (long)hit.Base - (long)block;
            string relative = offset >= 0 ? $"+0x{offset:x}" : $"-0x{-offset:x}";
            ulong reaches = hit.Base + hit.Size;
            text.Append(CultureInfo.InvariantCulture, $"\n    recording {hit.Serial} operation {hit.Index} ({stage}, {where}): 0x{hit.Base:x} to 0x{reaches:x} (block {relative}, 0x{hit.Size:x} bytes), dwords {hit.D0:x8} {hit.D1:x8} {hit.D2:x8} {hit.D3:x8}, pipeline {pipelineName(hit.Pipeline)}");
            printed++;
        }

        if (printed == 0)
        {
            text.Append("\n    none");
        }
    }
}
