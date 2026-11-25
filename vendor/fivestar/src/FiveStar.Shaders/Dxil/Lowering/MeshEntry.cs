// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// A mesh entry point: thread and group ids as in compute, and the <c>out vertices</c>, <c>out indices</c> and
/// <c>out primitives</c> arrays as write-only places. A write to a vertex or primitive becomes storeVertexOutput or
/// storePrimitiveOutput per component, a write of a whole index vector emitIndices; the output arrays' lengths and
/// the index vector's size give the limits and the topology.
/// </summary>
public sealed class MeshEntry
{
    private const string VerticesModifier = "vertices";
    private const string IndicesModifier = "indices";
    private const string PrimitivesModifier = "primitives";

    // DXIL MeshOutputTopology: 1 line, 2 triangle; an index vector has one index per corner.
    private const int LineTopology = 1;
    private const int TriangleTopology = 2;
    private const int LineCorners = 2;
    private const int TriangleCorners = 3;

    private readonly LoweringContext _context;
    private readonly SignatureBinder _binder;
    private readonly ComputeEntry _compute;
    private readonly List<DxilSignatureElement> _outputs = [];
    private readonly List<DxilSignatureElement> _primitives = [];
    private readonly Dictionary<string, (string Modifier, EntryBinding? Binding)> _arrays = new(StringComparer.Ordinal);

    public MeshEntry(LoweringContext context)
    {
        _context = context;
        _binder = new SignatureBinder(context);
        _compute = new ComputeEntry(context);
    }

    public IReadOnlyList<DxilSignatureElement> Outputs => _outputs;

    public IReadOnlyList<DxilSignatureElement> Primitives => _primitives;

    public int MaxVertices { get; private set; }

    public int MaxPrimitives { get; private set; }

    public int Topology { get; private set; } = TriangleTopology;

    private DxilModule Module => _context.Module;

    /// <summary>Builds and packs the vertex and primitive signatures from the output parameters.</summary>
    public void Declare(SyntaxNode main)
    {
        foreach (var parameter in Parameters(main))
        {
            var modifier = OutputModifier(parameter);

            if (modifier is null)
            {
                continue;
            }

            var type = Resolve(parameter);

            if (type.Shape != TypeShape.Array)
            {
                throw SourceError.At(parameter, $"mesh output {parameter.Text} is not an array");
            }

            _arrays[parameter.Text] = (modifier, DeclareArray(parameter, modifier, type));
        }

        SignaturePacker.Pack(_outputs, SignaturePoint.MeshVertexOutput);
        SignaturePacker.Pack(_primitives, SignaturePoint.MeshPrimitiveOutput);
    }

    /// <summary>Declares the id parameters with their values and the output arrays as places.</summary>
    public void BindParameters(SyntaxNode main)
    {
        foreach (var parameter in Parameters(main))
        {
            var type = Resolve(parameter);

            if (_arrays.ContainsKey(parameter.Text))
            {
                _context.Scope.Declare(new Variable(parameter.Text, type, StorageKind.MeshOutput));

                continue;
            }

            var variable = _context.Statements.DeclareLocal(parameter.Text, type);
            _context.Variables.Write(_context.Variables.Whole(variable), _compute.SystemValue(parameter, type));
        }
    }

    /// <summary>Writes <paramref name="value"/> to a place in one of the output arrays.</summary>
    public void Write(LValue place, RValue value)
    {
        var (modifier, binding) = _arrays[place.Variable.Name];
        var perElement = place.Variable.Type.Element!.Count;

        if (modifier == IndicesModifier)
        {
            EmitIndices(place, value, perElement);

            return;
        }

        var (code, name) = (DxOpCode.StoreVertexOutput, "storeVertexOutput");

        if (modifier == PrimitivesModifier)
        {
            (code, name) = (DxOpCode.StorePrimitiveOutput, "storePrimitiveOutput");
        }

        for (var index = 0; index < place.Type.Count; index++)
        {
            var (element, component) = SignatureBinder.Locate(Module, place, index);
            var slot = binding!.Holding(component);
            var stored = SignatureBinder.Stored(_context, value, index, slot.Kind);
            _context.Ops.Overloaded(
                code, name, AttributeSet.NoUnwind, stored.Type, Module.Types.Void,
                Module.Int32((uint)slot.Element.Id), Module.Int32(0), Module.Int8((byte)(component - slot.First)), stored, element);
        }
    }

    /// <summary><c>SetMeshOutputCounts(vertices, primitives)</c>.</summary>
    public RValue SetOutputCounts(SyntaxNode call)
    {
        var vertices = _context.Expressions.Lower(call[0], HlslType.Uint).Scalar;
        var primitives = _context.Expressions.Lower(call[1], HlslType.Uint).Scalar;
        _context.Ops.Call(DxOpCode.SetMeshOutputCounts, "setMeshOutputCounts", AttributeSet.NoUnwind, Module.Types.Void, vertices, primitives);

        return RValue.Void;
    }

    /// <summary>Sets each element's usage from the stores left after dead-code removal.</summary>
    public void RecordUsage(IrFunction main)
    {
        SignatureBinder.RecordUsage(main, new Dictionary<DxOpCode, IReadOnlyList<DxilSignatureElement>>
        {
            [DxOpCode.StoreVertexOutput] = _outputs,
            [DxOpCode.StorePrimitiveOutput] = _primitives,
        });
    }

    private EntryBinding? DeclareArray(SyntaxNode parameter, string modifier, HlslType type)
    {
        var element = type.Element!;

        switch (modifier)
        {
            case VerticesModifier:
                MaxVertices = type.Length;

                return _binder.Bind(parameter, element, _outputs, SignaturePoint.MeshVertexOutput);
            case PrimitivesModifier:
                MaxPrimitives = type.Length;

                return _binder.Bind(parameter, element, _primitives, SignaturePoint.MeshPrimitiveOutput);
            default:
                MaxPrimitives = type.Length;
                Topology = element.Count switch
                {
                    LineCorners => LineTopology,
                    TriangleCorners => TriangleTopology,
                    _ => throw SourceError.At(parameter, $"mesh indices of {element}"),
                };

                return null;
        }
    }

    /// <summary>emitIndices for a whole index vector.</summary>
    private void EmitIndices(LValue place, RValue value, int perElement)
    {
        if (place.Type.Count != perElement)
        {
            throw new DxilBackendException("a mesh index vector must be written whole");
        }

        var (primitive, _) = SignatureBinder.Locate(Module, place, 0);
        var corners = value.Components.Select(component => _context.Convert.Convert(RValue.Of(HlslType.Uint, component), HlslType.Uint).Scalar);
        _context.Ops.Call(DxOpCode.EmitIndices, "emitIndices", AttributeSet.NoUnwind, Module.Types.Void, [primitive, .. corners]);
    }

    private static IEnumerable<SyntaxNode> Parameters(SyntaxNode main)
    {
        return main.Children.Where(child => child.Kind == SyntaxKind.Parameter);
    }

    private static string? OutputModifier(SyntaxNode parameter)
    {
        string[] modifiers = [VerticesModifier, IndicesModifier, PrimitivesModifier];

        return modifiers.FirstOrDefault(parameter.HasModifier);
    }

    private HlslType Resolve(SyntaxNode parameter)
    {
        return _context.Types.Resolve(parameter.Type!, dimension => (int)_context.Expressions.ConstantInteger(dimension!));
    }
}
