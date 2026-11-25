// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Lowers a parsed program into a DXIL module: declarations, the entry block (globals, the user-data handle, the entry
/// parameters: thread ids, or the input signature of a graphics stage), the body with every call inlined, the outputs
/// at the exit, then SSA cleanup and the flags the module needs.
/// </summary>
public static class ShaderLowering
{
    private const string EntryName = "main";
    private const string HelperLanesAttribute = "WaveOpsIncludeHelperLanes";

    public static (DxilModule Module, IrFunction Main, ShaderDescription Description) Compute(IReadOnlyList<SyntaxNode> declarations)
    {
        return Lower(declarations, DxilShaderKind.Compute);
    }

    public static (DxilModule Module, IrFunction Main, ShaderDescription Description) Lower(IReadOnlyList<SyntaxNode> declarations, DxilShaderKind kind)
    {
        var module = new DxilModule();
        var types = module.Types;
        var main = module.Function(EntryName, types.Function(types.Void, []), AttributeSet.None, hasBody: true);
        var placeholder = module.NewBlock();
        var context = new LoweringContext(module, placeholder);
        var entry = context.Ssa.NewBlock();

        context.Ssa.Seal(entry);
        context.Builder.Block = entry;
        context.EntryBlock = entry;
        context.Flow.Entry = entry;

        var globals = new GlobalDeclarations(context);
        globals.Collect(declarations);
        var source = Entry(context);
        context.MemoryLocals = DxilBackend.MemoryLocalsCases > 0 && MostCases(source) >= DxilBackend.MemoryLocalsCases;
        globals.Declare(declarations);
        context.Scope.Push();

        var description = kind switch
        {
            DxilShaderKind.Compute => ComputeBody(context, source),
            DxilShaderKind.Mesh => MeshBody(context, source),
            DxilShaderKind.Geometry => GeometryBody(context, source),
            _ => GraphicsBody(context, source, kind),
        };
        context.Scope.Pop();

        context.Ssa.Finish();
        main.Blocks.AddRange(context.Ssa.Blocks);

        if (DxilBackend.Optimize)
        {
            ValueNumbering.Run(main);
        }

        DeadCode.Remove(main);

        if (DxilBackend.Optimize)
        {
            Simplifier.Run(main, module);
        }

        context.Graphics?.RecordUsage(main);
        context.Mesh?.RecordUsage(main);
        context.Geometry?.RecordUsage(main);
        var flags = context.Flags | FeatureScan.Flags(main) | SignatureFlags(context.Graphics);

        if (description.IncludeHelperLanes)
        {
            main.Attribute = AttributeSet.HelperLanes;
        }

        return (module, main, description with { Flags = flags });
    }

    private static ShaderDescription ComputeBody(LoweringContext context, SyntaxNode source)
    {
        var computeEntry = new ComputeEntry(context);
        var threads = computeEntry.Threads(source);
        var waveSize = computeEntry.WaveSize(source);
        computeEntry.BindParameters(source);
        Body(context, source);

        return new ShaderDescription(threads, waveSize, ShaderFlagBits.None, context.UserDataBytes);
    }

    private static ShaderDescription MeshBody(LoweringContext context, SyntaxNode source)
    {
        var computeEntry = new ComputeEntry(context);
        var threads = computeEntry.Threads(source);
        var waveSize = computeEntry.WaveSize(source);
        var mesh = new MeshEntry(context);
        context.Mesh = mesh;
        mesh.Declare(source);

        mesh.BindParameters(source);
        Body(context, source);

        return new ShaderDescription(threads, waveSize, ShaderFlagBits.None, context.UserDataBytes)
        {
            Kind = DxilShaderKind.Mesh,
            Outputs = mesh.Outputs,
            Primitives = mesh.Primitives,
            Mesh = new MeshState(mesh.MaxVertices, mesh.MaxPrimitives, mesh.Topology, context.SharedBytes),
        };
    }

    private static ShaderDescription GeometryBody(LoweringContext context, SyntaxNode source)
    {
        var geometry = new GeometryEntry(context);
        context.Geometry = geometry;
        geometry.Declare(source);
        geometry.BindParameters(source);
        Body(context, source);

        return new ShaderDescription((0, 0, 0), null, ShaderFlagBits.None, context.UserDataBytes)
        {
            Kind = DxilShaderKind.Geometry,
            Inputs = geometry.Inputs,
            Outputs = geometry.Outputs,
            Geometry = geometry.State,
        };
    }

    private static ShaderDescription GraphicsBody(LoweringContext context, SyntaxNode source, DxilShaderKind kind)
    {
        var graphics = new GraphicsEntry(context, kind);
        context.Graphics = graphics;
        graphics.Declare(source);
        graphics.BindParameters(source);
        Body(context, source);

        return new ShaderDescription((0, 0, 0), null, ShaderFlagBits.None, context.UserDataBytes)
        {
            Kind = kind,
            Inputs = graphics.Inputs,
            Outputs = graphics.Outputs,
            IncludeHelperLanes = source.Attributes.Any(attribute => attribute.Text == HelperLanesAttribute),
        };
    }

    /// <summary>Flags the signatures need: barycentrics, stencil export.</summary>
    private static ShaderFlagBits SignatureFlags(GraphicsEntry? graphics)
    {
        var flags = ShaderFlagBits.None;

        if (graphics is null)
        {
            return flags;
        }

        if (graphics.Inputs.Any(element => element.Kind == SemanticKind.Barycentrics))
        {
            flags |= ShaderFlagBits.Barycentrics;
        }

        if (graphics.Outputs.Any(element => element.Kind == SemanticKind.StencilRef))
        {
            flags |= ShaderFlagBits.StencilExport;
        }

        return flags;
    }

    /// <summary>The most cases one switch statement under <paramref name="node"/> has.</summary>
    private static int MostCases(SyntaxNode node)
    {
        int most = 0;

        if (node.Kind == SyntaxKind.Switch)
        {
            most = node.Children.Concat(node.BodyOrEmpty).Count(child => child.Kind == SyntaxKind.Case);
        }

        foreach (var child in node.Children.Concat(node.BodyOrEmpty))
        {
            most = Math.Max(most, MostCases(child));
        }

        return most;
    }

    private static SyntaxNode Entry(LoweringContext context)
    {
        if (!context.Functions.TryGetValue(EntryName, out var overloads))
        {
            throw new DxilBackendException("no entry function main");
        }

        return overloads[0];
    }

    /// <summary>The entry's body; its returned value (a graphics stage's outputs) is stored at the exit.</summary>
    private static void Body(LoweringContext context, SyntaxNode main)
    {
        var flow = context.Flow;
        var exit = flow.NewBlock();
        var body = main.Children.Last(child => child.Kind == SyntaxKind.Block);
        var returnType = context.Types.Resolve(main.Type!, _ => throw SourceError.At(main, "an array return type is not supported"));
        var returnSlots = new List<int>();

        if (returnType.Shape != TypeShape.Void)
        {
            var variable = context.Variables.DeclareSlots(EntryName + ".return", returnType);
            returnSlots.AddRange(Enumerable.Range(variable.FirstSlot, returnType.Count));
        }

        flow.PushFrame(new FunctionFrame(exit, returnType, returnSlots));
        context.Statements.LowerBlockContents(body);
        flow.Jump(exit);

        flow.PopFrame();
        flow.Seal(exit);
        flow.Enter(exit);

        if (returnSlots.Count > 0)
        {
            var value = returnSlots.Select(slot => context.Ssa.Read(slot, context.Builder.Block)).ToArray();
            context.Graphics?.StoreOutputs(new RValue(returnType, value));
        }

        context.Builder.Return();
    }
}
