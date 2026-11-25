// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Loader.Diagnostics;
using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Translation;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The window guard (on unless <c>--no-window-guard</c>): a program's store or atomic through a 32-bit pointer (high
/// dword 0) would land in the descriptor window, whose records and debug rings programs have no business writing; the
/// prelude (FIVESTAR_WINDOW_GUARD) drops it and counts it in slot <c>id &amp; 1023</c> of a table past the room for view
/// records: {program id (<see cref="MeshProbe.Id"/> of its name, FIVESTAR_PROGRAM_ID), drops}; an atomic goes to the
/// sink dword after the table. At every captured present the programs that dropped stores since the last are logged.
/// Such stores come from pointers built from a V# base that holds a heap index, and from 32-bit table pointers. A cap
/// table follows (<see cref="CapDefine"/>): per program {id, threads past the superstep block cap, groups past the
/// round bound, waves past the loop cap}, logged likewise. A cap only keeps the device alive while a wrong input or a
/// translation bug runs a loop on; every hit is named so that its cause is found and fixed.
/// </summary>
internal static unsafe class WindowGuard
{
    /// <summary>The define of guarded programs; its value is the table's window offset.</summary>
    public const string Define = "FIVESTAR_WINDOW_GUARD";

    /// <summary>The define of a program's id, which its drops are counted under.</summary>
    public const string ProgramDefine = "FIVESTAR_PROGRAM_ID";

    /// <summary>The table's window offset: right past the room for view records.</summary>
    public const uint Base = DescriptorWindow.RecordsEnd;

    /// <summary>The define of the cap table's window offset (rdna_prelude.hlsli, ss_cap_hit).</summary>
    public const string CapDefine = "FIVESTAR_CAP_TABLE";

    /// <summary>The cap table's offset from <see cref="Base"/>: past the store table and its sink.</summary>
    public const uint CapOffset = 0x2100;

    /// <summary>The store table, its sink and the cap table (4 dwords a slot).</summary>
    public const uint RegionBytes = CapOffset + (Slots * CapSlotDwords * sizeof(uint));

    /// <summary>Window bytes the guard needs.</summary>
    public const uint Bytes = Base + RegionBytes;

    private const uint Slots = 1024;

    /// <summary>A slot: the program id, then its drops.</summary>
    private const uint SlotDwords = 2;

    /// <summary>A cap slot: the program id, threads past the block cap, groups past the round bound, waves past the loop cap.</summary>
    private const uint CapSlotDwords = 4;
    private const int BlockDword = 1;
    private const int RoundDword = 2;
    private const int LoopDword = 3;
    private const int Shown = 12;
    private const string Prefix = "  [window guard] ";

    private static readonly uint[] Reported = new uint[Slots];
    private static readonly CapCounts ReportedCaps = new(new uint[Slots], new uint[Slots], new uint[Slots]);
    private static readonly object CapGate = new();
    private static Dictionary<uint, string>? _names;

    /// <summary>Whether programs made on demand drop their stores into the window.</summary>
    public static bool Enabled { get; set; } = true;

    /// <summary>The extracted programs, for the names of the ids in the table.</summary>
    public static ProgramIndex? Index { get; set; }

    /// <summary>The mode define (<see cref="Pipelines.ProgramProducer.ModeHeader"/>).</summary>
    public static string Header => $"#define {Define} {Base}u\n#define {CapDefine} {Base + CapOffset}u\n";

    /// <summary>The define of <paramref name="name"/>'s id, for a program made with the guard (its header holds <see cref="Define"/>).</summary>
    public static string ProgramHeader(string header, string name)
    {
        if (!header.Contains(Define, StringComparison.Ordinal))
        {
            return string.Empty;
        }

        return $"#define {ProgramDefine} {MeshProbe.Id(DebugLayer.MeshProbes.Source(name))}u\n";
    }

    /// <summary>Logs the programs whose stores into the window were dropped since the last report; the GPU is idle.</summary>
    public static void Report(long present, LoaderLog log)
    {
        byte* window = ShimContext.Runtime.D3D12X.Bindings.Window.Data;

        if (!Enabled || window == null)
        {
            return;
        }

        var dropped = new List<(uint Id, uint Count)>();
        uint* table = (uint*)(window + Base);

        for (uint slot = 0; slot < Slots; slot++)
        {
            uint count = table[(slot * SlotDwords) + 1];
            uint fresh = count - Reported[slot];
            Reported[slot] = count;

            if (fresh != 0)
            {
                dropped.Add((table[slot * SlotDwords], fresh));
            }
        }

        if (dropped.Count == 0)
        {
            log.Info($"{Prefix}present {present}: no program stored into the window");

            return;
        }

        var names = Names();
        var text = new StringBuilder($"{Prefix}present {present}: {dropped.Count} programs stored into the window (dropped):");

        foreach (var (id, count) in dropped.OrderByDescending(entry => entry.Count).Take(Shown))
        {
            text.Append($"\n{Prefix}  {count} by {names.GetValueOrDefault(id, $"program 0x{id:x8}")}");
        }

        log.Info(text.ToString());
    }

    /// <summary>
    /// Logs the programs that ran into a cap since the last report: threads past the superstep block cap, groups past
    /// the round bound and waves past the loop cap, by name; nothing when none did. Called at captured presents (the GPU
    /// idle) and at every thread sample (the GPU running: counts as they stand, the rest reported next time), so a boot
    /// that never reaches a capture still names its runaway programs.
    /// </summary>
    public static void ReportCaps(long present, LoaderLog log)
    {
        byte* window = ShimContext.Runtime.D3D12X.Bindings.Window.Data;

        if (!Enabled || window == null)
        {
            return;
        }

        ReportCaps((uint*)(window + Base + CapOffset), present, log);
    }

    /// <summary>The report of <see cref="ReportCaps(long, LoaderLog)"/> from the cap <paramref name="table"/>; the presenter and the sampler take turns.</summary>
    public static void ReportCaps(uint* table, long present, LoaderLog log)
    {
        List<CapHit> hits;

        lock (CapGate)
        {
            hits = CapHits(table, ReportedCaps);
        }

        if (hits.Count == 0)
        {
            return;
        }

        var names = Names();
        var text = new StringBuilder($"{Prefix}present {present}: {hits.Count} programs ran into a cap (a wrong input or translation to fix):");

        foreach (var hit in hits.OrderByDescending(hit => (long)hit.Blocks + hit.Rounds + hit.Loops).Take(Shown))
        {
            var name = names.GetValueOrDefault(hit.Id, $"program 0x{hit.Id:x8}");
            text.Append($"\n{Prefix}  {name}: {hit.Blocks} threads past the block cap, {hit.Rounds} groups past the round bound, {hit.Loops} waves past the loop cap");
        }

        log.Warn(text.ToString());
    }

    /// <summary>
    /// The programs of the cap <paramref name="table"/> whose counts grew since <paramref name="seen"/> (the counts last
    /// seen, updated): the program id and the new block-cap threads, round-bound groups and loop-cap waves.
    /// </summary>
    public static List<CapHit> CapHits(uint* table, CapCounts seen)
    {
        var hits = new List<CapHit>();

        for (uint slot = 0; slot < Slots; slot++)
        {
            uint* entry = table + (slot * CapSlotDwords);
            var hit = new CapHit(entry[0], entry[BlockDword] - seen.Blocks[slot], entry[RoundDword] - seen.Rounds[slot], entry[LoopDword] - seen.Loops[slot]);
            (seen.Blocks[slot], seen.Rounds[slot], seen.Loops[slot]) = (entry[BlockDword], entry[RoundDword], entry[LoopDword]);

            if (hit.Blocks != 0 || hit.Rounds != 0 || hit.Loops != 0)
            {
                hits.Add(hit);
            }
        }

        return hits;
    }

    private static Dictionary<uint, string> Names()
    {
        // Built whole before it is published: the presenter and the sampler both read it.
        if (_names is null && Index is not null)
        {
            var names = new Dictionary<uint, string>();

            foreach (var name in Index.Names)
            {
                names[MeshProbe.Id(DebugLayer.MeshProbes.Source(name))] = DebugLayer.MeshProbes.Source(name);
            }

            _names = names;
        }

        return _names ?? [];
    }
}
