// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// The state of one program translation: the statements emitted so far, which registers exist, which hold lane masks
/// and known constants, and what the program declares (inputs, attributes, exports, static samplers).
/// </summary>
internal sealed class TranslationState
{
    private const int Wave64 = 64;

    // Mesh shaders without numthreads in their PSV0 run 128 threads (NGG groups hold up to 128 vertices).
    private const int DefaultMeshThreads = 128;

    // Compute programs without numthreads in their header run 64 threads (as the entry point declares).
    private const int DefaultComputeThreads = 64;

    private int _temporaries;
    private HashSet<(int Address, int Dword)>? _samplerLoads;

    public TranslationState(ShaderProgram program, IReadOnlyList<Instruction> instructions, ShaderStage stage, bool copyShader)
    {
        Stage = stage;
        Instructions = [.. instructions];
        Code = program.Code;
        Signatures = program.Signatures;
        Config = program.Config;
        PipelineState = program.PipelineState;

        IsCopyShader = copyShader;

        var threads = program.PipelineState?.NumThreads;
        var meshThreads = threads is null ? DefaultMeshThreads : threads.Value.X;
        MeshThreads = stage == ShaderStage.Mesh ? meshThreads : 0;
        GroupThreads = MeshThreads;

        if (stage == ShaderStage.Compute)
        {
            var compute = program.Config.Compute?.NumThreads;
            GroupThreads = DefaultComputeThreads;

            if (compute is not null)
            {
                GroupThreads = compute.Value.X * compute.Value.Y * compute.Value.Z;
            }
        }

        Masks.Add(SpecialRegisters.Exec);
    }

    public ShaderStage Stage { get; }

    /// <summary>The instructions; long jumps are rewritten in place.</summary>
    public List<Instruction> Instructions { get; }

    public byte[]? Code { get; }

    public ShaderSignatures Signatures { get; }

    public ShaderConfig Config { get; }

    public PipelineStateInfo? PipelineState { get; }

    /// <summary>True while translating the VS copy shader of a geometry program (emitted as a function).</summary>
    public bool IsCopyShader { get; }

    public int MeshThreads { get; }

    /// <summary>The threads of a mesh or compute thread group (0 for other stages).</summary>
    public int GroupThreads { get; }

    public bool IsWave64 => Config.WaveSize == Wave64;

    /// <summary>The console wave size the program was compiled for.</summary>
    public int WaveSize => Config.WaveSize ?? Wave64;

    /// <summary>Wave operations keep console lane numbers on any hardware wave size (see TranslationOptions).</summary>
    public bool EmulateWaves { get; init; }

    /// <summary>A mesh program the window analysis rejects may keep the native form, approximately (TranslationOptions.FastWaves).</summary>
    public bool FastWaves { get; init; }

    /// <summary>Image loads, gathers and stores follow the view's component class (TranslationOptions.TypedImageViews).</summary>
    public bool TypedImageViews { get; init; }

    /// <summary>The loop caps (TranslationOptions.Loops, else the translator's own).</summary>
    public LoopCaps Loops { get; init; } = LoopCaps.Default;

    /// <summary>Superstep form (mesh or compute): wave operations exchange through groupshared memory, one group step each.</summary>
    public bool Supersteps { get; init; }

    /// <summary>Superstep form: a step deposits four readfirstlane values (rdna_superstep.hlsli's VW_SS_VALUES).</summary>
    public bool SuperstepValues { get; set; }

    /// <summary>Superstep form: registers pass between cases through carry slots (TranslationOptions.SuperstepCarry).</summary>
    public bool SuperstepCarry { get; init; }

    /// <summary>Superstep form: temporaries declared at function scope because a step cuts their block.</summary>
    public List<string> Hoisted { get; } = [];

    /// <summary>
    /// The widest aligned group of program lanes one cross-lane read may reach: a hardware wave at least this wide gives
    /// console results under wave emulation (<see cref="WaveOps.FloorLine"/>).
    /// </summary>
    public int LaneReach { get; private set; } = 1;

    /// <summary>SGPRs holding the number of a lane taken from a lane mask (s_ff1): that lane is always present.</summary>
    public HashSet<RegisterKey> MaskLanes { get; } = [];

    /// <summary>Hardware-preloaded inputs, assigned before the first block.</summary>
    public List<string> Prologue { get; } = [];

    public List<CodeBlock> Blocks { get; } = [];

    /// <summary>The statements of the block being translated.</summary>
    public List<string> Body { get; set; } = [];

    public bool HasControlFlow { get; set; }

    /// <summary>Every register the program touches (declared as uint).</summary>
    public HashSet<RegisterKey> Variables { get; } = [];

    /// <summary>Registers written before the current point; reading any other register reads a hardware input.</summary>
    public HashSet<RegisterKey> Written { get; } = [];

    /// <summary>Registers currently holding a lane mask (a per-lane bool); EXEC starts as one.</summary>
    public HashSet<RegisterKey> Masks { get; } = [];

    /// <summary>Registers that ever held a lane mask (their bools are declared).</summary>
    public HashSet<RegisterKey> MasksEver { get; } = [];

    /// <summary>Wave64: SGPRs holding the high dword of a lane mask.</summary>
    public HashSet<RegisterKey> MaskHighHalves { get; } = [];

    /// <summary>SGPRs used as integers (written, or read while not a mask) somewhere in the program.</summary>
    public HashSet<RegisterKey> IntegerUses { get; } = [];

    /// <summary>The form each block reads and leaves its SGPRs in (Stages.MaskFormAnalysis).</summary>
    public MaskForms Forms { get; } = new();

    /// <summary>
    /// SGPRs that are a lane mask on some paths and an integer on others (found by a first translation): their mask
    /// writes also set the integer variables and every read uses those, so a read is right whichever path ran
    /// (Stages.ProgramTranslator).
    /// </summary>
    public IReadOnlySet<RegisterKey> Coherent { get; init; } = new HashSet<RegisterKey>();

    /// <summary>Registers holding a known constant in the current block (for literal descriptors).</summary>
    public Dictionary<RegisterKey, long> Constants { get; } = [];

    /// <summary>VGPR number to the system input it holds (vid, iid, bary, pos_x, ...).</summary>
    public Dictionary<int, string> Inputs { get; } = [];

    /// <summary>Pixel-shader attribute number to how it is read (linear, flat, vtx1, vtx2, f16lo, f16hi).</summary>
    public Dictionary<int, HashSet<string>> Attributes { get; } = [];

    /// <summary>Export targets written (mrt0, mrtz, pos0, param3, ...).</summary>
    public HashSet<string> Outputs { get; } = [];

    /// <summary>Mesh staging: export target to the number of components written.</summary>
    public Dictionary<string, int> MeshWidths { get; } = [];

    /// <summary>The S#s built from literal constants, in first-use order (TranslationOptions-independent slots, see StaticSampler).</summary>
    public List<StaticSampler> StaticSamplers { get; } = [];

    /// <summary>Vertex attributes read through the fetch shader: (attribute, component count).</summary>
    public HashSet<(int Attribute, int Components)> FetchInputs { get; } = [];

    /// <summary>The program starts with a call into the driver's vertex fetch subroutine.</summary>
    public bool UsesFetchShader { get; set; }

    /// <summary>Superstep form: group rounds per wave-operation stop (<see cref="TranslationOptions.RoundsPerStop"/>); null for the caps' bound.</summary>
    public int? RoundsPerStop { get; init; }

    /// <summary>The address of the instruction being translated.</summary>
    public int Address { get; set; }

    /// <summary>The s_load dwords that load an S#'s dword 0 (<see cref="SamplerLoads"/>).</summary>
    public HashSet<(int Address, int Dword)> SamplerLoadDwords => _samplerLoads ??= SamplerLoads.Find(Instructions);

    /// <summary>The program reads its own code bytes (s_getpc_b64 pointers).</summary>
    public bool UsesCode { get; set; }

    public bool UsesBvh { get; set; }

    public bool UsesLds { get; set; }

    /// <summary>Mesh shader in two-phase direct-output mode (s_barrier becomes a yield point).</summary>
    public bool MeshYield { get; set; }

    /// <summary>The program ends a primitive-ordered (POPS) critical section.</summary>
    public bool Pops { get; set; }

    /// <summary>DPP: the bool that says whether this lane writes its destination (else it keeps its old value).</summary>
    public string? DppKeep { get; set; }

    /// <summary>Lines that assemble attrN values from the input signature (set when emitting).</summary>
    public List<string> InputMap { get; } = [];

    /// <summary>Lines that write signature outputs from the export registers (set when emitting).</summary>
    public List<string> OutputMap { get; } = [];

    /// <summary>Appends one statement to the current block.</summary>
    public void Emit(string line)
    {
        Body.Add(line);
    }

    /// <summary>A fresh number for a temporary name.</summary>
    public int NextTemporary()
    {
        var number = _temporaries;
        _temporaries++;

        return number;
    }

    /// <summary>Marks a register as written and present.</summary>
    public void Touch(RegisterKey key)
    {
        Written.Add(key);
        Variables.Add(key);
    }

    /// <summary>Records a cross-lane read that stays inside aligned groups of <paramref name="lanes"/> program lanes.</summary>
    public void Reach(int lanes)
    {
        LaneReach = Math.Max(LaneReach, Math.Min(lanes, WaveSize));
    }

    /// <summary>Records a read of a lane that may be anywhere in the program wave.</summary>
    public void ReachWholeWave()
    {
        Reach(WaveSize);
    }

    /// <summary>Records a read of the fixed program lane <paramref name="lane"/>.</summary>
    public void ReachLane(long lane)
    {
        if (Stage == ShaderStage.Mesh)
        {
            // A narrower mesh hardware wave holds a window that starts anywhere in the program wave.
            ReachWholeWave();

            return;
        }

        Reach((int)System.Numerics.BitOperations.RoundUpToPowerOf2((uint)(lane & (WaveSize - 1)) + 1));
    }

    /// <summary>Records a read of the lane number held in <paramref name="lane"/> (an SGPR, or any expression).</summary>
    public void ReachLane(RegisterKey? lane)
    {
        if (lane is not null && MaskLanes.Contains(lane.Value) && Stage != ShaderStage.Mesh)
        {
            return;
        }

        ReachWholeWave();
    }

    /// <summary>Records how a pixel-shader attribute is read.</summary>
    public void AddAttributeMode(int attribute, string mode)
    {
        if (!Attributes.TryGetValue(attribute, out var modes))
        {
            modes = [];
            Attributes[attribute] = modes;
        }

        modes.Add(mode);
    }
}
