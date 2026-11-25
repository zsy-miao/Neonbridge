// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>The state one compile shares between the lowering parts: module, builders, names, flags.</summary>
public sealed class LoweringContext
{
    public LoweringContext(DxilModule module, BasicBlock entry)
    {
        Module = module;
        Builder = new IrBuilder(module, entry) { Fold = DxilBackend.Optimize };
        Ssa = new SsaBuilder(module);
        Ops = new DxOps(Builder);
        Convert = new Conversions(Builder);
        Flow = new ControlFlow(Builder, Ssa);

        Expressions = new ExpressionLowerer(this);
        Statements = new StatementLowerer(this);
        Intrinsics = new IntrinsicLowerer(this);
        Resources = new ResourceLowerer(this);
        Variables = new VariableStore(this);
    }

    public DxilModule Module { get; }

    public IrBuilder Builder { get; }

    public SsaBuilder Ssa { get; }

    public DxOps Ops { get; }

    public Conversions Convert { get; }

    public ControlFlow Flow { get; }

    public TypeResolver Types { get; } = new();

    public Scope Scope { get; } = new();

    public ExpressionLowerer Expressions { get; }

    public StatementLowerer Statements { get; }

    public IntrinsicLowerer Intrinsics { get; }

    public ResourceLowerer Resources { get; }

    public VariableStore Variables { get; }

    /// <summary>User functions by name (overloads in declaration order).</summary>
    public Dictionary<string, List<SyntaxNode>> Functions { get; } = new(StringComparer.Ordinal);

    /// <summary>Struct declarations by name (their fields' semantics and modifiers).</summary>
    public Dictionary<string, SyntaxNode> StructDeclarations { get; } = new(StringComparer.Ordinal);

    /// <summary>The signatures of a graphics entry point (null for compute).</summary>
    public GraphicsEntry? Graphics { get; set; }

    /// <summary>The outputs of a mesh entry point (null for other stages).</summary>
    public MeshEntry? Mesh { get; set; }

    /// <summary>The inputs and output stream of a geometry entry point (null for other stages).</summary>
    public GeometryEntry? Geometry { get; set; }

    /// <summary>The block allocas go to.</summary>
    public BasicBlock? EntryBlock { get; set; }

    public ShaderFlagBits Flags { get; set; }

    /// <summary>
    /// Whether numeric locals live in memory (an alloca each) instead of SSA slots: for a program with a dispatch switch
    /// of thousands of cases (the superstep form), where the joins of every register made phis with thousands of
    /// operands (<see cref="DxilBackend.MemoryLocalsCases"/>).
    /// </summary>
    public bool MemoryLocals { get; set; }

    /// <summary>The user-data constant buffer's handle, made on first use in the entry block.</summary>
    public Value? UserDataHandle { get; set; }

    public int UserDataBytes { get; set; }

    /// <summary>How many inlined calls are open (recursion guard).</summary>
    public int CallDepth { get; set; }

    /// <summary>Thread-group shared memory declared, in bytes.</summary>
    public int SharedBytes { get; set; }
}
