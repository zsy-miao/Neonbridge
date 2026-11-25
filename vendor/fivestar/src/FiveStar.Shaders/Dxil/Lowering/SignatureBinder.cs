// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Turns entry parameters and return values into signature elements (a value with a semantic, or each field of a
/// struct), and reads back which components the finished module loads and stores.
/// </summary>
public sealed class SignatureBinder
{
    public const int NoId = -1;

    // loadInput, storeOutput, storeVertexOutput, storePrimitiveOutput: opcode, element id, row, column, ...
    private const int IdOperand = 1;
    private const int ColumnOperand = 3;

    private readonly LoweringContext _context;

    public SignatureBinder(LoweringContext context)
    {
        _context = context;
    }

    /// <summary>The elements of a parameter or the return value, added to <paramref name="elements"/>.</summary>
    public EntryBinding Bind(SyntaxNode node, HlslType type, List<DxilSignatureElement> elements, SignaturePoint point)
    {
        var binding = new EntryBinding(type);

        if (type.Shape != TypeShape.Struct)
        {
            var semantic = node.Semantic ?? throw SourceError.At(node, $"{node.Text} has no semantic");
            binding.Elements.Add(Element(node, semantic, type, node.Modifiers, elements, point, 0));

            return binding;
        }

        var declaration = _context.StructDeclarations[type.Name];
        var first = 0;

        for (var index = 0; index < type.Fields.Count; index++)
        {
            var field = declaration.Children[index];
            var fieldType = type.Fields[index].Type;
            var semantic = field.Semantic ?? throw SourceError.At(field, $"field {field.Text} has no semantic");
            var modifiers = new HashSet<string>(field.Modifiers, StringComparer.Ordinal);
            modifiers.UnionWith(node.Modifiers);

            binding.Elements.Add(Element(field, semantic, fieldType, modifiers, elements, point, first));
            binding.Fields[field.Text] = binding.Elements[^1];
            first += fieldType.Count;
        }

        return binding;
    }

    /// <summary>Component <paramref name="index"/> of <paramref name="value"/> as an output of <paramref name="kind"/> stores it (bools as uint).</summary>
    public static Value Stored(LoweringContext context, RValue value, int index, ScalarKind kind)
    {
        var source = RValue.Of(HlslType.ScalarOf(value.Type.ComponentKind(index)), value.Components[index]);
        var stored = kind;

        if (kind == ScalarKind.Bool)
        {
            stored = ScalarKind.Uint;
        }

        return context.Convert.Convert(source, HlslType.ScalarOf(stored)).Scalar;
    }

    /// <summary>
    /// The array element (vertex, primitive) and the component in it that component <paramref name="index"/> of a place
    /// in a signature array (mesh outputs, geometry inputs) names.
    /// </summary>
    public static (Value Element, int Component) Locate(DxilModule module, LValue place, int index)
    {
        var flat = (int)((Constant)place.Indices[index]).Bits;

        if (place.ArrayIndex is not null)
        {
            return (place.ArrayIndex, flat);
        }

        var perElement = place.Variable.Type.Element!.Count;

        return (module.Int32((uint)(flat / perElement)), flat % perElement);
    }

    /// <summary>Sets each element's usage from the loads and stores (by operation) left after dead-code removal.</summary>
    public static void RecordUsage(IrFunction main, IReadOnlyDictionary<DxOpCode, IReadOnlyList<DxilSignatureElement>> signatures)
    {
        foreach (var instruction in main.Blocks.SelectMany(block => block.Body))
        {
            if (instruction.Opcode != Opcode.Call || instruction.Operands.Count <= ColumnOperand || instruction.Operands[0] is not Constant code)
            {
                continue;
            }

            if (!signatures.TryGetValue((DxOpCode)code.Bits, out var elements))
            {
                continue;
            }

            var id = (int)((Constant)instruction.Operands[IdOperand]).Bits;
            var column = (int)((Constant)instruction.Operands[ColumnOperand]).Bits;
            var element = elements.First(candidate => candidate.Id == id);
            element.Usage |= (byte)(1 << column);
        }
    }

    private static ElementSlot Element(
        SyntaxNode node, string semantic, HlslType type, IReadOnlySet<string> modifiers, List<DxilSignatureElement> elements, SignaturePoint point, int first)
    {
        if (type.Shape is not (TypeShape.Scalar or TypeShape.Vector))
        {
            throw SourceError.At(node, $"signature element {semantic} of type {type} is not supported");
        }

        var (name, index, kind) = Semantics.Parse(semantic);
        var interpretation = Semantics.Interpret(kind, point);
        var id = NoId;

        if (interpretation != SemanticInterpretation.NotInSignature)
        {
            id = elements.Count;
        }

        var element = new DxilSignatureElement
        {
            Id = id,
            Name = name,
            Index = index,
            Kind = kind,
            Interpretation = interpretation,
            ComponentType = ComponentType(type.Kind, kind, node),
            Interpolation = Semantics.Interpolation(point, kind, type.Kind == ScalarKind.Float, modifiers),
            Columns = type.Count,
        };

        if (element.InSignature)
        {
            elements.Add(element);
        }

        return new ElementSlot(element, first, type.Kind);
    }

    /// <summary>The element's DXIL component type (bools as uint, except the cull flag, which is i1).</summary>
    private static byte ComponentType(ScalarKind scalar, SemanticKind kind, SyntaxNode node)
    {
        if (kind == SemanticKind.CullPrimitive)
        {
            return DxilSignatureElement.Bool;
        }

        return scalar switch
        {
            ScalarKind.Float => DxilSignatureElement.Float32,
            ScalarKind.Int => DxilSignatureElement.Int32,
            ScalarKind.Uint or ScalarKind.Bool => DxilSignatureElement.Uint32,
            _ => throw SourceError.At(node, $"signature element of {scalar} is not supported"),
        };
    }
}
