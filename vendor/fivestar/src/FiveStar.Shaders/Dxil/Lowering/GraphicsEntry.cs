// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// The signatures of a pixel or vertex entry point: every parameter with a semantic (or struct of such fields) is an
/// input element, every field of the returned struct an output element, packed as dxc packs them. Inputs are loaded
/// in the entry block (unused loads are dead code), outputs stored at the exit, and the components the module still
/// reads and writes become each element's usage mask.
/// </summary>
public sealed class GraphicsEntry
{
    private const string LoadInputClass = "loadInput";
    private const string StoreOutputClass = "storeOutput";
    private const string AttributeAtVertexClass = "attributeAtVertex";
    private const string Positions = "xyzw";
    private const string Colors = "rgba";
    private const int MaxSwizzle = 4;

    private readonly LoweringContext _context;
    private readonly SignatureBinder _binder;
    private readonly SignaturePoint _inputPoint;
    private readonly SignaturePoint _outputPoint;
    private readonly List<DxilSignatureElement> _inputs = [];
    private readonly List<DxilSignatureElement> _outputs = [];
    private readonly Dictionary<string, EntryBinding> _parameters = new(StringComparer.Ordinal);
    private EntryBinding? _result;

    public GraphicsEntry(LoweringContext context, DxilShaderKind kind)
    {
        _context = context;
        _binder = new SignatureBinder(context);
        (_inputPoint, _outputPoint) = kind switch
        {
            DxilShaderKind.Pixel => (SignaturePoint.PixelInput, SignaturePoint.PixelOutput),
            DxilShaderKind.Vertex => (SignaturePoint.VertexInput, SignaturePoint.VertexOutput),
            _ => throw new DxilBackendException($"{kind} shaders are not supported"),
        };
    }

    public IReadOnlyList<DxilSignatureElement> Inputs => _inputs;

    public IReadOnlyList<DxilSignatureElement> Outputs => _outputs;

    private DxilModule Module => _context.Module;

    /// <summary>Builds and packs both signatures from the entry's parameters and return type.</summary>
    public void Declare(SyntaxNode main)
    {
        foreach (var parameter in main.Children.Where(child => child.Kind == SyntaxKind.Parameter))
        {
            var type = Resolve(parameter.Type!, parameter);
            _parameters[parameter.Text] = _binder.Bind(parameter, type, _inputs, _inputPoint);
        }

        var returnType = Resolve(main.Type!, main);

        if (returnType.Shape != TypeShape.Void)
        {
            _result = _binder.Bind(main, returnType, _outputs, _outputPoint);
        }

        SignaturePacker.Pack(_inputs, _inputPoint);
        SignaturePacker.Pack(_outputs, _outputPoint);
    }

    /// <summary>Declares each parameter and fills it from its elements.</summary>
    public void BindParameters(SyntaxNode main)
    {
        foreach (var parameter in main.Children.Where(child => child.Kind == SyntaxKind.Parameter))
        {
            var binding = _parameters[parameter.Text];
            var variable = _context.Statements.DeclareLocal(parameter.Text, binding.Type);
            var components = new Value[binding.Type.Count];

            foreach (var (element, first, kind) in binding.Elements)
            {
                for (var column = 0; column < element.Columns; column++)
                {
                    components[first + column] = Load(element, column, kind);
                }
            }

            var loaded = new RValue(binding.Type, components);
            _context.Variables.Write(_context.Variables.Whole(variable), loaded);
        }
    }

    /// <summary>Stores the entry's returned value to the output elements.</summary>
    public void StoreOutputs(RValue value)
    {
        if (_result is null)
        {
            return;
        }

        foreach (var (element, first, kind) in _result.Elements)
        {
            for (var column = 0; column < element.Columns; column++)
            {
                var component = SignatureBinder.Stored(_context, value, first + column, kind);
                _context.Ops.Overloaded(
                    DxOpCode.StoreOutput, StoreOutputClass, AttributeSet.NoUnwind, component.Type, Module.Types.Void,
                    Module.Int32((uint)element.Id), Module.Int32(0), Module.Int8((byte)column), component);
            }
        }
    }

    /// <summary>
    /// <c>GetAttributeAtVertex(input, vertex)</c>: the attribute as the given vertex of the primitive wrote it, read from
    /// the input element <paramref name="input"/> names (a parameter or one of its fields, with an optional swizzle).
    /// </summary>
    public RValue AttributeAtVertex(SyntaxNode input, Value vertex)
    {
        var (element, kind, columns) = Find(input);

        if (vertex is not Constant constant)
        {
            throw SourceError.At(input, "GetAttributeAtVertex needs a constant vertex");
        }

        var vertexIndex = Module.Int8((byte)constant.Bits);
        var overload = kind == ScalarKind.Float ? Module.Types.Float : Module.Types.Int32;
        var components = columns.Select(column => _context.Ops.Overloaded(
            DxOpCode.AttributeAtVertex, AttributeAtVertexClass, AttributeSet.ReadNone, overload, overload,
            Module.Int32((uint)element.Id), Module.Int32(0), Module.Int8((byte)column), vertexIndex)).ToArray();
        var loadedKind = kind == ScalarKind.Float ? ScalarKind.Float : ScalarKind.Uint;

        return _context.Convert.Convert(new RValue(HlslType.Vector(loadedKind, components.Length), components), HlslType.Vector(kind, components.Length));
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

    private Value Load(DxilSignatureElement element, int column, ScalarKind kind)
    {
        var types = Module.Types;
        var ops = _context.Ops;

        if (element.Kind == SemanticKind.Coverage)
        {
            return ops.Overloaded(DxOpCode.Coverage, "coverage", AttributeSet.ReadNone, types.Int32, types.Int32);
        }

        if (element.Kind == SemanticKind.InnerCoverage)
        {
            return ops.Overloaded(DxOpCode.InnerCoverage, "innerCoverage", AttributeSet.ReadNone, types.Int32, types.Int32);
        }

        if (element.Kind == SemanticKind.SampleIndex)
        {
            return ops.Overloaded(DxOpCode.SampleIndex, "sampleIndex", AttributeSet.ReadNone, types.Int32, types.Int32);
        }

        var overload = kind == ScalarKind.Float ? types.Float : types.Int32;
        var value = ops.Overloaded(
            DxOpCode.LoadInput, LoadInputClass, AttributeSet.ReadNone, overload, overload,
            Module.Int32((uint)element.Id), Module.Int32(0), Module.Int8((byte)column), Module.Undefined(types.Int32));

        if (kind == ScalarKind.Bool)
        {
            return _context.Builder.Compare(Predicate.NotEqual, value, Module.Int32(0));
        }

        return value;
    }

    private HlslType Resolve(TypeSyntax syntax, SyntaxNode node)
    {
        return _context.Types.Resolve(syntax, _ => throw SourceError.At(node, "array signature elements are not supported"));
    }

    /// <summary>The input element a GetAttributeAtVertex argument names, its scalar kind and the columns it reads.</summary>
    private (DxilSignatureElement Element, ScalarKind Kind, int[] Columns) Find(SyntaxNode input)
    {
        var node = input;
        string? swizzle = null;

        if (node.Kind == SyntaxKind.Member && IsSwizzle(node.Text) && node[0].Kind == SyntaxKind.Member)
        {
            swizzle = node.Text;
            node = node[0];
        }

        ElementSlot? found = null;

        if (node.Kind == SyntaxKind.Member && node[0].Kind == SyntaxKind.Name && _parameters.TryGetValue(node[0].Text, out var owner)
            && owner.Fields.TryGetValue(node.Text, out var field))
        {
            found = field;
        }
        else if (node.Kind == SyntaxKind.Name && _parameters.TryGetValue(node.Text, out var parameter) && parameter.Elements.Count == 1)
        {
            found = parameter.Elements[0];
        }

        if (found is null || found.Value.Element.Id == SignatureBinder.NoId)
        {
            throw SourceError.At(input, "GetAttributeAtVertex needs an input element");
        }

        var entry = found.Value;
        int[] columns = [.. Enumerable.Range(0, entry.Element.Columns)];

        if (swizzle is not null)
        {
            columns = [.. swizzle.Select(SwizzleComponent)];
        }

        return (entry.Element, entry.Kind, columns);
    }

    private static bool IsSwizzle(string text)
    {
        return text.Length is > 0 and <= MaxSwizzle && text.All(letter => SwizzleComponent(letter) >= 0);
    }

    /// <summary>The component a swizzle letter selects (xyzw or rgba), or -1.</summary>
    private static int SwizzleComponent(char letter)
    {
        var component = Positions.IndexOf(letter, StringComparison.Ordinal);

        if (component < 0)
        {
            component = Colors.IndexOf(letter, StringComparison.Ordinal);
        }

        return component;
    }
}
