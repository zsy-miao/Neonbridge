// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// A geometry entry point: the input vertex array (<c>triangle GSIn gin[3]</c>) read through loadInput with the vertex
/// index, the primitive id, and the output stream whose <c>Append</c> stores every output element and emits a vertex
/// and whose <c>RestartStrip</c> cuts the strip. <c>[maxvertexcount(n)]</c>, the input primitive and the stream's
/// type give the geometry state.
/// </summary>
public sealed class GeometryEntry
{
    private const string MaxVertexCountAttribute = "maxvertexcount";
    private const string StreamSuffix = "Stream";
    private const int StreamMask = 1;

    // DXIL InputPrimitive and PrimitiveTopology (DxilConstants.h).
    private static readonly Dictionary<string, int> InputPrimitives = new(StringComparer.Ordinal)
    {
        ["point"] = 1,
        ["line"] = 2,
        ["triangle"] = 3,
        ["lineadj"] = 6,
        ["triangleadj"] = 7,
    };

    private static readonly Dictionary<string, int> Topologies = new(StringComparer.Ordinal)
    {
        ["PointStream"] = 1,
        ["LineStream"] = 3,
        ["TriangleStream"] = 5,
    };

    private readonly LoweringContext _context;
    private readonly SignatureBinder _binder;
    private readonly List<DxilSignatureElement> _inputs = [];
    private readonly List<DxilSignatureElement> _outputs = [];
    private readonly Dictionary<string, EntryBinding> _parameters = new(StringComparer.Ordinal);
    private string? _vertices;
    private string? _stream;

    public GeometryEntry(LoweringContext context)
    {
        _context = context;
        _binder = new SignatureBinder(context);
    }

    public IReadOnlyList<DxilSignatureElement> Inputs => _inputs;

    public IReadOnlyList<DxilSignatureElement> Outputs => _outputs;

    public GeometryState State { get; private set; } = new(0, 0, 0, StreamMask);

    private DxilModule Module => _context.Module;

    /// <summary>Builds and packs both signatures and reads the geometry state.</summary>
    public void Declare(SyntaxNode main)
    {
        var attribute = main.Attributes.FirstOrDefault(candidate => candidate.Text == MaxVertexCountAttribute)
            ?? throw SourceError.At(main, "a geometry entry needs [maxvertexcount]");
        State = State with { MaxVertexCount = (int)_context.Expressions.ConstantInteger(attribute[0]) };

        foreach (var parameter in Parameters(main))
        {
            var type = Resolve(parameter);
            var primitive = parameter.Modifiers.FirstOrDefault(InputPrimitives.ContainsKey);

            if (primitive is not null)
            {
                _vertices = parameter.Text;
                State = State with { InputPrimitive = InputPrimitives[primitive] };
                _parameters[parameter.Text] = _binder.Bind(parameter, type.Element!, _inputs, SignaturePoint.GeometryVertexInput);
            }
            else if (type.Shape == TypeShape.Resource && type.Name.EndsWith(StreamSuffix, StringComparison.Ordinal))
            {
                _stream = parameter.Text;
                State = State with { OutputTopology = Topologies[type.Name] };
                _parameters[parameter.Text] = _binder.Bind(parameter, type.Element!, _outputs, SignaturePoint.GeometryOutput);
            }
            else
            {
                _parameters[parameter.Text] = _binder.Bind(parameter, type, _inputs, SignaturePoint.GeometryInput);
            }
        }

        SignaturePacker.Pack(_inputs, SignaturePoint.GeometryVertexInput);
        SignaturePacker.Pack(_outputs, SignaturePoint.GeometryOutput);
    }

    /// <summary>Declares the vertex array as a place and the primitive id with its value.</summary>
    public void BindParameters(SyntaxNode main)
    {
        foreach (var parameter in Parameters(main))
        {
            if (parameter.Text == _stream)
            {
                continue;
            }

            var type = Resolve(parameter);

            if (parameter.Text == _vertices)
            {
                _context.Scope.Declare(new Variable(parameter.Text, type, StorageKind.GeometryInput));

                continue;
            }

            var variable = _context.Statements.DeclareLocal(parameter.Text, type);
            _context.Variables.Write(_context.Variables.Whole(variable), SystemValue(parameter, type));
        }
    }

    /// <summary>Whether <paramref name="name"/> is the output stream.</summary>
    public bool IsStream(string name)
    {
        return name == _stream;
    }

    /// <summary><c>stream.Append(vertex)</c> or <c>stream.RestartStrip()</c>.</summary>
    public RValue Method(SyntaxNode call)
    {
        var stream = Module.Int8(0);

        if (call.Text == "RestartStrip")
        {
            _context.Ops.Call(DxOpCode.CutStream, "cutStream", AttributeSet.NoUnwind, Module.Types.Void, stream);

            return RValue.Void;
        }

        if (call.Text != "Append")
        {
            throw SourceError.At(call, $"stream method {call.Text} is not supported");
        }

        var binding = _parameters[_stream!];
        var vertex = _context.Expressions.Lower(call[1], binding.Type);

        foreach (var (element, first, kind) in binding.Elements)
        {
            for (var column = 0; column < element.Columns; column++)
            {
                var component = SignatureBinder.Stored(_context, vertex, first + column, kind);
                _context.Ops.Overloaded(
                    DxOpCode.StoreOutput, "storeOutput", AttributeSet.NoUnwind, component.Type, Module.Types.Void,
                    Module.Int32((uint)element.Id), Module.Int32(0), Module.Int8((byte)column), component);
            }
        }

        _context.Ops.Call(DxOpCode.EmitStream, "emitStream", AttributeSet.NoUnwind, Module.Types.Void, stream);

        return RValue.Void;
    }

    /// <summary>Reads a place in the input vertex array: loadInput per component with the vertex index.</summary>
    public RValue Read(LValue place)
    {
        var binding = _parameters[_vertices!];
        var components = new Value[place.Type.Count];
        var types = Module.Types;

        for (var index = 0; index < components.Length; index++)
        {
            var (vertex, component) = SignatureBinder.Locate(Module, place, index);
            var slot = binding.Holding(component);
            var overload = slot.Kind == ScalarKind.Float ? types.Float : types.Int32;
            var value = _context.Ops.Overloaded(
                DxOpCode.LoadInput, "loadInput", AttributeSet.ReadNone, overload, overload,
                Module.Int32((uint)slot.Element.Id), Module.Int32(0), Module.Int8((byte)(component - slot.First)), vertex);
            var loaded = RValue.Of(HlslType.ScalarOf(Loaded(slot.Kind)), value);
            components[index] = _context.Convert.Convert(loaded, HlslType.ScalarOf(place.Type.ComponentKind(index))).Scalar;
        }

        return new RValue(place.Type, components);
    }

    /// <summary>Sets each element's usage from the loads and stores left after dead-code removal.</summary>
    public void RecordUsage(IrFunction main)
    {
        SignatureBinder.RecordUsage(main, new Dictionary<DxOpCode, IReadOnlyList<DxilSignatureElement>>
        {
            [DxOpCode.LoadInput] = _inputs,
            [DxOpCode.StoreOutput] = _outputs,
        });
    }

    private RValue SystemValue(SyntaxNode parameter, HlslType type)
    {
        var element = _parameters[parameter.Text].Elements[0].Element;
        var int32 = Module.Types.Int32;
        Value value;

        switch (element.Kind)
        {
            case SemanticKind.PrimitiveId:
                value = _context.Ops.Overloaded(DxOpCode.PrimitiveId, "primitiveID", AttributeSet.ReadNone, int32, int32);
                break;
            case SemanticKind.GsInstanceId:
                value = _context.Ops.Overloaded(DxOpCode.GsInstanceId, "gsInstanceID", AttributeSet.ReadNone, int32, int32);
                break;
            default:
                throw SourceError.At(parameter, $"geometry input {parameter.Text} is not supported");
        }

        return _context.Convert.Convert(RValue.Of(HlslType.Uint, value), type);
    }

    /// <summary>The kind a loadInput returns for an element of <paramref name="kind"/>: float, or the 32-bit integer.</summary>
    private static ScalarKind Loaded(ScalarKind kind)
    {
        if (kind is ScalarKind.Float or ScalarKind.Int)
        {
            return kind;
        }

        return ScalarKind.Uint;
    }

    private static IEnumerable<SyntaxNode> Parameters(SyntaxNode main)
    {
        return main.Children.Where(child => child.Kind == SyntaxKind.Parameter);
    }

    private HlslType Resolve(SyntaxNode parameter)
    {
        return _context.Types.Resolve(parameter.Type!, dimension => (int)_context.Expressions.ConstantInteger(dimension!));
    }
}
