// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// <c>--d3d12-trace-records &lt;offset,...&gt;</c>: for each draw the trace logs by pipeline name, the window records a
/// program reads at those byte offsets from every user SGPR that holds a window offset (a descriptor pointer the game
/// passes as a root constant, which the root signature's tables do not show). Each record's dwords, then for a V# its
/// base (or heap index), whether the address map has the base's page and the first dwords there; for a T# its base and
/// map state; and the game's own record at the same heap index, to tell a record the window lacks from one the game
/// never wrote. An entry <c>offset@byte</c> also shows the dword at that byte of the V#'s buffer.
/// </summary>
internal static unsafe class TraceRecords
{
    /// <summary>Dwords shown of a record (a T# has 8; a V# uses the first 4).</summary>
    public const int RecordDwords = 8;

    /// <summary>Dwords shown at a V#'s base: a constant buffer's first values.</summary>
    public const int BaseDwords = 10;

    private const uint RecordBytes = 32;
    private const int TypeShift = 28;

    /// <summary>Record dwords: base low, base high (V#: and stride), V# num_records, then the dword with the type.</summary>
    private const int BaseHigh = 1;
    private const int Records = 2;
    private const int TypeDword = 3;

    private const int HexPrefixLength = 2;

    /// <summary>T# types start at 8 in dword 3 bits 28-31 (SQ_RSRC_IMG_*); below is a V# (SQ_RSRC_BUF).</summary>
    private const uint FirstImageType = 8;

    private const uint BaseHighMask = 0xFFFF;
    private const uint ImageHighMask = 0xFF;
    private const int ImageBaseShift = 8;
    private const int DwordBits = 32;

    private const char DwordSeparator = '@';

    /// <summary>The entries; empty for none.</summary>
    public static IReadOnlyList<TraceRecord> Offsets { get; set; } = [];

    /// <summary>
    /// Entries of a comma-separated list: <c>offset</c> or <c>offset@byte</c>, each number hexadecimal with 0x or decimal.
    /// </summary>
    public static IReadOnlyList<TraceRecord> Parse(string list)
    {
        return [.. list.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).Select(ParseEntry)];
    }

    private static TraceRecord ParseEntry(string text)
    {
        var at = text.IndexOf(DwordSeparator, StringComparison.Ordinal);

        if (at < 0)
        {
            return new TraceRecord(ParseOne(text));
        }

        return new TraceRecord(ParseOne(text[..at].Trim()), ParseOne(text[(at + 1)..].Trim()));
    }

    /// <summary>
    /// The records at <paramref name="offsets"/> from each distinct user SGPR of <paramref name="sgprs"/> that is a
    /// record-aligned offset with room in the <paramref name="windowBytes"/> of view records at <paramref name="window"/>.
    /// </summary>
    public static void Append(StringBuilder text, byte* window, uint windowBytes, ReadOnlySpan<uint> sgprs, IReadOnlyList<TraceRecord> offsets, TraceRecordSources sources)
    {
        if (window == null || offsets.Count == 0)
        {
            return;
        }

        uint last = offsets.Max(entry => entry.Offset);
        var seen = new HashSet<uint>();

        for (int sgpr = 0; sgpr < sgprs.Length; sgpr++)
        {
            uint pointer = sgprs[sgpr];

            if (pointer == 0 || pointer % RecordBytes != 0 || (ulong)pointer + last + RecordBytes > windowBytes || !seen.Add(pointer))
            {
                continue;
            }

            foreach (var entry in offsets)
            {
                var record = (uint*)(window + pointer + entry.Offset);
                text.Append(CultureInfo.InvariantCulture, $"\n      sgpr {sgpr} 0x{pointer:x} +0x{entry.Offset:x}:");
                AppendRecord(text, record, sources, entry.Dword);
                AppendGameRecord(text, pointer + entry.Offset, sources.GameRecord);
            }
        }
    }

    /// <summary>The game's own record at the heap index of window offset <paramref name="offset"/>.</summary>
    private static void AppendGameRecord(StringBuilder text, uint offset, Func<uint, nint> gameRecord)
    {
        uint index = (offset - Binding.DescriptorWindow.ViewBase) / RecordBytes;
        var record = (uint*)gameRecord(offset);

        if (record == null)
        {
            text.Append(CultureInfo.InvariantCulture, $"\n        game record {index}: (no game heap)");

            return;
        }

        text.Append(CultureInfo.InvariantCulture, $"\n        game record {index}:");
        AppendDwords(text, record);
    }

    private static void AppendDwords(StringBuilder text, uint* record)
    {
        for (int dword = 0; dword < RecordDwords; dword++)
        {
            text.Append(CultureInfo.InvariantCulture, $" {record[dword]:x8}");
        }
    }

    private static void AppendRecord(StringBuilder text, uint* record, TraceRecordSources sources, uint? dword)
    {
        var mapState = sources.MapState;
        var memory = sources.Memory;
        AppendDwords(text, record);

        if (record[TypeDword] >> TypeShift >= FirstImageType)
        {
            ulong image = (record[0] | ((ulong)(record[BaseHigh] & ImageHighMask) << DwordBits)) << ImageBaseShift;
            text.Append(CultureInfo.InvariantCulture, $"\n        T# base 0x{image:x}: {mapState(image)}; {sources.Texture(image)}");

            return;
        }

        uint high = record[BaseHigh] & BaseHighMask;

        if (high == 0)
        {
            text.Append(CultureInfo.InvariantCulture, $"\n        heap index {record[0]}, no console base: {sources.View(record[0])}");

            return;
        }

        ulong address = record[0] | ((ulong)high << DwordBits);
        text.Append(CultureInfo.InvariantCulture, $"\n        V# base 0x{address:x}, {record[Records]} records: {mapState(address)}; first dwords:");
        var data = (uint*)memory(address, BaseDwords * sizeof(uint));

        if (data == null)
        {
            text.Append(" (memory not readable)");
        }
        else
        {
            for (int i = 0; i < BaseDwords; i++)
            {
                text.Append(CultureInfo.InvariantCulture, $" {data[i]:x8}");
            }
        }

        if (dword.HasValue)
        {
            AppendDword(text, address, dword.Value, memory);
        }
    }

    /// <summary>The dword at byte <paramref name="at"/> of the buffer at <paramref name="address"/>.</summary>
    private static void AppendDword(StringBuilder text, ulong address, uint at, Func<ulong, int, nint> memory)
    {
        var value = (uint*)memory(address + at, sizeof(uint));

        if (value == null)
        {
            text.Append(CultureInfo.InvariantCulture, $"; dword at +0x{at:x}: (memory not readable)");

            return;
        }

        text.Append(CultureInfo.InvariantCulture, $"; dword at +0x{at:x}: {*value:x8} ({*value})");
    }

    private static uint ParseOne(string text)
    {
        if (text.StartsWith("0x", StringComparison.OrdinalIgnoreCase))
        {
            return uint.Parse(text.AsSpan(HexPrefixLength), NumberStyles.HexNumber, CultureInfo.InvariantCulture);
        }

        return uint.Parse(text, CultureInfo.InvariantCulture);
    }
}
