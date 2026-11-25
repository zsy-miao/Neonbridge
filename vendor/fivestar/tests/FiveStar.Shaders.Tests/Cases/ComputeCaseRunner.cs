// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// Runs a compute case: adds the store epilogue, assembles (wave64 programs with +wavefrontsize64), translates with the
/// case's compute layout, compiles (cs_6_7), dispatches on WARP and compares every thread. Wave-level expectations are
/// evaluated per PC wave: AMD wave32 / wave64 semantics modelled on the PC wave's lanes, results that would come from
/// lanes outside it are don't-care.
/// </summary>
public sealed class ComputeCaseRunner
{
    private const int FirstResult = 20;

    private readonly WarpFixture _fixture;

    /// <summary>The probe ring's heap slot (the writable window, rdna_prelude GPU_MEM_HEAP + 1) and its records.</summary>
    private const int ProbeRingSlot = 1;
    private const int ProbeRecords = 64;

    public ComputeCaseRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    public IReadOnlyList<string> Run(ComputeCase testCase)
    {
        return Run(testCase, null);
    }

    /// <summary>
    /// Runs a wave64 case through its group-wave variant (HlslTranslator.GroupWaveVariant): the 64-thread group is the
    /// console wave, so every thread is compared with full-wave expectations.
    /// </summary>
    public IReadOnlyList<string> RunGroupWave(ComputeCase testCase)
    {
        return Run(testCase, GroupWave);
    }

    /// <summary>
    /// Runs a case in the superstep form (HlslTranslator.SuperstepVariant) with groups of two console waves, which
    /// branch apart: every thread is compared with full-wave expectations. <paramref name="carry"/> adds the liveness
    /// carry (TranslationOptions.SuperstepCarry).
    /// </summary>
    public IReadOnlyList<string> RunSupersteps(ComputeCase testCase, bool carry)
    {
        var options = new TranslationOptions(StructuredControlFlow: false, PinWaveSize: false, EmulateWaveSize: true) { SuperstepCarry = carry };

        return Run(testCase with { Threads = (2 * testCase.Wave, 1, 1) }, program => HlslTranslator.SuperstepVariant(program, options));
    }

    /// <summary>Runs a case with <paramref name="defines"/> (HLSL #define lines) before its translation, as the loader's mode header.</summary>
    public IReadOnlyList<string> RunDefined(ComputeCase testCase, string defines)
    {
        return Run(testCase, program => defines + ShaderWorkbench.Translate(program));
    }

    /// <summary>Runs a case compiled by the DXIL backend instead of dxc (the same translation, the same expectations).</summary>
    public IReadOnlyList<string> RunDirect(ComputeCase testCase)
    {
        return Run(testCase, null, direct: true);
    }

    /// <summary>
    /// Runs a case with the stage probe (MeshProbe on a compute program, its ring at the start of the writable window,
    /// heap slot 1): the case's mismatches, the records its waves wrote and the text of each load site.
    /// </summary>
    public (IReadOnlyList<string> Mismatches, IReadOnlyList<MeshProbeRecord> Records, IReadOnlyList<string> Sites) RunProbed(ComputeCase testCase, uint id)
    {
        IReadOnlyList<string> sites = [];
        testCase = testCase with { Name = testCase.Name + "_probe" };
        var shader = Build(testCase, program =>
        {
            var (probed, numbered) = MeshProbe.Instrument(ShaderWorkbench.Translate(program), id, 0);
            sites = numbered;

            return probed;
        }, direct: false);
        uint[] user = [.. ComputeCase.Descriptor(ComputeCase.OutputHeap), .. testCase.SecondDescriptor, .. testCase.User, .. new uint[ComputeCase.UserSgprCount]];
        var output = new BufferResource(ComputeCase.OutputHeap, [.. Enumerable.Repeat(testCase.Fill, testCase.TotalThreads * testCase.Outputs)], Writable: true);

        var ring = new BufferResource(ProbeRingSlot, new uint[(MeshProbe.RecordsOffset / sizeof(uint)) + (ProbeRecords * MeshProbeRecord.Dwords)], Writable: true);
        var bindings = new Bindings { UserSgprs = user[..ComputeCase.UserSgprCount], Buffers = [output, ring, .. testCase.Buffers], Textures = testCase.Textures };
        var (x, y, z) = testCase.Groups;
        var result = _fixture.Runner.Compute(new ComputeJob(shader, x, y, z, bindings));

        var words = result.Buffers[ProbeRingSlot];
        var records = Enumerable.Range(0, (int)Math.Min(words[0], ProbeRecords))
            .Select(index => MeshProbeRecord.Read(words.AsSpan((int)(MeshProbe.RecordsOffset / sizeof(uint)) + (index * MeshProbeRecord.Dwords), MeshProbeRecord.Dwords)))
            .ToList();
        var written = result.Buffers.Concat(result.Textures).ToDictionary(pair => pair.Key, pair => pair.Value);

        return (Compare(testCase, written, Math.Min(_fixture.ComputeWaveSize, testCase.Wave)), records, sites);
    }

    /// <summary>
    /// Runs a case with the cap table (rdna_prelude FIVESTAR_CAP_TABLE at the start of the writable window, heap slot 1)
    /// under program id <paramref name="id"/>: the case's mismatches and the four dwords of the program's slot.
    /// </summary>
    public (IReadOnlyList<string> Mismatches, uint[] Slot) RunCapCounted(ComputeCase testCase, uint id)
    {
        const int SlotDwords = 4;
        const int Slots = 1024;
        var defines = $"#define FIVESTAR_PROGRAM_ID {id}u\n#define FIVESTAR_CAP_TABLE 0u\n";
        var shader = Build(testCase with { Name = testCase.Name + "_caps" }, program => defines + ShaderWorkbench.Translate(program), direct: false);

        uint[] user = [.. ComputeCase.Descriptor(ComputeCase.OutputHeap), .. testCase.SecondDescriptor, .. testCase.User, .. new uint[ComputeCase.UserSgprCount]];
        var output = new BufferResource(ComputeCase.OutputHeap, [.. Enumerable.Repeat(testCase.Fill, testCase.TotalThreads * testCase.Outputs)], Writable: true);
        var table = new BufferResource(ProbeRingSlot, new uint[Slots * SlotDwords], Writable: true);
        var bindings = new Bindings { UserSgprs = user[..ComputeCase.UserSgprCount], Buffers = [output, table, .. testCase.Buffers], Textures = testCase.Textures };

        var (x, y, z) = testCase.Groups;
        var result = _fixture.Runner.Compute(new ComputeJob(shader, x, y, z, bindings));
        var written = result.Buffers.Concat(result.Textures).ToDictionary(pair => pair.Key, pair => pair.Value);
        var start = (int)(id & (Slots - 1)) * SlotDwords;

        return (Compare(testCase, written, Math.Min(_fixture.ComputeWaveSize, testCase.Wave)), result.Buffers[ProbeRingSlot][start..(start + SlotDwords)]);
    }

    private static string GroupWave(ShaderProgram program)
    {
        // As the corpus builds it: structured (WARP's shader compiler crashes on barriers in the dispatch switch).
        var structured = HlslTranslator.Translate(program, new TranslationOptions(StructuredControlFlow: true, PinWaveSize: true, EmulateWaveSize: true));

        return HlslTranslator.GroupWaveVariant(structured.Hlsl);
    }

    private List<string> Run(ComputeCase testCase, Func<ShaderProgram, string>? variant)
    {
        return Run(testCase, variant, direct: false);
    }

    private List<string> Run(ComputeCase testCase, Func<ShaderProgram, string>? variant, bool direct)
    {
        var shader = Build(testCase, variant, direct);
        uint[] user = [.. ComputeCase.Descriptor(ComputeCase.OutputHeap), .. testCase.SecondDescriptor, .. testCase.User, .. new uint[ComputeCase.UserSgprCount]];
        var output = new BufferResource(ComputeCase.OutputHeap, [.. Enumerable.Repeat(testCase.Fill, testCase.TotalThreads * testCase.Outputs)], Writable: true);
        var bindings = new Bindings { UserSgprs = user[..ComputeCase.UserSgprCount], Buffers = [output, .. testCase.Buffers], Textures = testCase.Textures };
        var (x, y, z) = testCase.Groups;
        var result = _fixture.Runner.Compute(new ComputeJob(shader, x, y, z, bindings));

        var wave = Math.Min(_fixture.ComputeWaveSize, testCase.Wave);

        if (variant is not null)
        {
            wave = testCase.Wave;
        }

        // Writable textures are read back beside the buffers, by heap slot, for Check.
        var written = result.Buffers.Concat(result.Textures).ToDictionary(pair => pair.Key, pair => pair.Value);

        return Compare(testCase, written, wave);
    }

    private byte[] Build(ComputeCase testCase, Func<ShaderProgram, string>? variant, bool direct)
    {
        var assembled = _fixture.Workbench.Assemble(testCase.Name, Source(testCase), testCase.Wave);
        var groupIds = testCase.GroupAxes.Select((axis, index) => new GroupIdSgpr(axis, ComputeCase.UserSgprCount + index)).ToList();
        var (x, y, z) = testCase.Threads;
        var compute = new ComputeConfig
        {
            Rsrc2 = 0,
            ScratchEnable = false,
            GroupSizeEnable = false,
            ThreadIdComponentCount = testCase.ThreadIdComponents,
            LdsBlocks = 0,
            NumThreads = new ThreadGroupSize(x, y, z),
            GroupIdSgprs = groupIds,
        };
        var config = new ShaderConfig
        {
            Stage = HardwareStage.Compute,
            WaveSize = testCase.Wave,
            UserSgprs = ComputeCase.UserSgprCount,
            LdsBytes = testCase.LdsBytes,
            Compute = compute,
        };
        var program = new ShaderProgram(ShaderStage.Compute, assembled.Instructions, assembled.Code, ShaderSignatures.None, config, null);

        if (variant is not null)
        {
            return _fixture.Workbench.Compile($"{testCase.Name}_w32_{testCase.ThreadsPerGroup}", variant(program), "cs");
        }

        if (direct)
        {
            return _fixture.Workbench.Compile(testCase.Name, ShaderWorkbench.Translate(program), "cs", direct: true);
        }

        return _fixture.Workbench.Compile(testCase.Name, ShaderWorkbench.Translate(program), "cs");
    }

    private static string Source(ComputeCase testCase)
    {
        var lines = new List<string> { $"v_mul_u32_u24_e32 v40, {4 * testCase.Outputs}, v41" };
        lines.AddRange(Enumerable.Range(0, testCase.Outputs).Select(k => $"buffer_store_dword v{FirstResult + k}, v40, s[0:3], 0 offen offset:{4 * k}"));
        var store = string.Join("\n", lines);

        if (testCase.Raw)
        {
            return testCase.Program.Replace("{STORE}", store, StringComparison.Ordinal) + "\ns_endpgm";
        }

        var prologue = $"s_mul_i32 s39, s12, {testCase.ThreadsPerGroup}\nv_add_nc_u32_e32 v41, s39, v0";

        return $"{prologue}\n{testCase.Program}\n{store}\ns_endpgm";
    }

    private static List<string> Compare(ComputeCase testCase, IReadOnlyDictionary<int, uint[]> buffers, int wave)
    {
        var mismatches = new List<string>();
        var output = buffers[ComputeCase.OutputHeap];

        for (var id = 0; testCase.Expect is not null && id < testCase.TotalThreads; id++)
        {
            var (group, index) = Math.DivRem(id, testCase.ThreadsPerGroup);
            var lane = index % wave;
            var thread = new ComputeThread(id, index, group, lane, wave, [.. Enumerable.Range(id - lane, wave)]);
            var expected = testCase.Expect(thread);

            for (var k = 0; k < expected.Length; k++)
            {
                var actual = output[(id * testCase.Outputs) + k];

                if (!expected[k].Matches(actual))
                {
                    mismatches.Add(string.Create(
                        CultureInfo.InvariantCulture,
                        $"thread {id} (group {group}, lane {lane}) v{FirstResult + k} expected {expected[k]} actual {Expected.Hex(actual)}"));
                }
            }
        }

        if (testCase.Check is not null)
        {
            mismatches.AddRange(testCase.Check(buffers));
        }

        return mismatches;
    }
}
