// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Lowers expressions to values and places. <c>&amp;&amp;</c>, <c>||</c> and <c>?:</c> on scalars evaluate their later
/// operands only when needed (HLSL 2021) unless those are free of effects, memory reads and wave operations, in which
/// case both sides are computed and combined.
/// </summary>
public sealed class ExpressionLowerer
{
    private const int FalseArm = 2;

    private readonly LoweringContext _context;
    private readonly Operators _operators;
    private readonly Purity _purity;

    public ExpressionLowerer(LoweringContext context)
    {
        _context = context;
        _operators = new Operators(context);
        _purity = new Purity(context);
    }

    public Operators Operators => _operators;

    private IrBuilder Builder => _context.Builder;

    private DxilModule Module => _context.Module;

    private VariableStore Variables => _context.Variables;

    public RValue Lower(SyntaxNode node)
    {
        switch (node.Kind)
        {
            case SyntaxKind.IntegerLiteral:
                return Integer(node.Literal);
            case SyntaxKind.RealLiteral:
                return Real(node.Literal);
            case SyntaxKind.BoolLiteral:
                return RValue.Of(HlslType.Bool, Module.Bool(node.Text == "true"));
            case SyntaxKind.Name:
            case SyntaxKind.Member:
            case SyntaxKind.Index:
                return Access(node);
            case SyntaxKind.Unary:
                return Unary(node);
            case SyntaxKind.PostIncrement:
                return Increment(node[0], node.Text, post: true);
            case SyntaxKind.Binary:
                return Binary(node);
            default:
                return LowerOther(node);
        }
    }

    private RValue LowerOther(SyntaxNode node)
    {
        switch (node.Kind)
        {
            case SyntaxKind.Assign:
                return Assign(node);
            case SyntaxKind.Conditional:
                return Conditional(node);
            case SyntaxKind.Call:
                return Call(node);
            case SyntaxKind.MethodCall:
                return Method(node);
            case SyntaxKind.Cast:
                return Cast(node);
            case SyntaxKind.InitializerList:
                return InitializerList(node);
            case SyntaxKind.Comma:
                return Comma(node);
            default:
                throw SourceError.At(node, $"{node.Kind} is not an expression");
        }
    }

    /// <summary>A method call: on a geometry shader's output stream, else on a resource.</summary>
    private RValue Method(SyntaxNode node)
    {
        var geometry = _context.Geometry;

        if (geometry is not null && node[0].Kind == SyntaxKind.Name && geometry.IsStream(node[0].Text))
        {
            return geometry.Method(node);
        }

        return _context.Resources.Method(node);
    }

    /// <summary>A scalar condition as an i1.</summary>
    public Value Condition(SyntaxNode node)
    {
        var value = Lower(node);

        if (value.Type.Count != 1)
        {
            throw SourceError.At(node, "a condition must be a scalar");
        }

        return _context.Convert.Convert(value, HlslType.Bool).Scalar;
    }

    /// <summary>An expression converted to <paramref name="type"/>.</summary>
    public RValue Lower(SyntaxNode node, HlslType type)
    {
        return _context.Convert.Convert(Lower(node), type);
    }

    /// <summary>A uint index or size.</summary>
    public Value Index(SyntaxNode node)
    {
        return Lower(node, HlslType.Uint).Scalar;
    }

    /// <summary>A compile-time integer (array sizes, case labels, attribute arguments).</summary>
    public long ConstantInteger(SyntaxNode node)
    {
        var value = Lower(node).Scalar;

        if (value is not Constant constant || constant.Kind != ConstantKind.Integer)
        {
            throw SourceError.At(node, "expected a constant integer");
        }

        return constant.Bits;
    }

    public bool IsPure(SyntaxNode node)
    {
        return _purity.IsPure(node);
    }

    private static RValue IntegerLiteral(Conversions convert, ScalarKind kind, long value)
    {
        return RValue.Of(HlslType.ScalarOf(kind), convert.IntegerConstant(kind, value));
    }

    private RValue Integer(Token token)
    {
        var kind = token.Suffix switch
        {
            'u' => ScalarKind.Uint,
            'l' => ScalarKind.Int64,
            _ => ScalarKind.LiteralInt,
        };

        if (kind == ScalarKind.Uint && token.Integer > uint.MaxValue)
        {
            kind = ScalarKind.Uint64;
        }

        return IntegerLiteral(_context.Convert, kind, (long)token.Integer);
    }

    private RValue Real(Token token)
    {
        var kind = token.Suffix switch
        {
            'f' or 'h' => ScalarKind.Float,
            'l' => ScalarKind.Double,
            _ => ScalarKind.LiteralFloat,
        };

        return RValue.Of(HlslType.ScalarOf(kind), _context.Convert.FloatConstant(kind, token.Real));
    }

    /// <summary>A name, member or element: read through its place when it has one.</summary>
    private RValue Access(SyntaxNode node)
    {
        if (node.Kind == SyntaxKind.Index && node[0].Kind == SyntaxKind.Name && _context.Resources.IsHeap(node[0].Text))
        {
            return ResourceLowerer.HeapReference(node[0].Text, Index(node[1]));
        }

        var texture = ResourceBase(node);

        if (texture is not null)
        {
            return _context.Resources.Textures.ReadElement(node, texture);
        }

        var place = TryPlace(node);

        if (place is not null)
        {
            return Variables.Read(place);
        }

        if (node.Kind == SyntaxKind.Name)
        {
            throw SourceError.At(node, $"unknown name {node.Text}");
        }

        return Part(Lower(node[0]), node);
    }

    /// <summary>The place an expression names; throws when it names none.</summary>
    public LValue Place(SyntaxNode node)
    {
        return TryPlace(node) ?? throw SourceError.At(node, "not assignable");
    }

    private LValue? TryPlace(SyntaxNode node)
    {
        switch (node.Kind)
        {
            case SyntaxKind.Name:
                return NamePlace(node);
            case SyntaxKind.Member:
            case SyntaxKind.Index:
                return Nested(node);
            default:
                return null;
        }
    }

    private LValue? NamePlace(SyntaxNode node)
    {
        var variable = _context.Scope.Find(node.Text);

        if (variable is null)
        {
            return null;
        }

        return Variables.Whole(variable);
    }

    /// <summary>A member or element of a place (no delegate: this runs for every variable access).</summary>
    private LValue? Nested(SyntaxNode node)
    {
        var owner = TryPlace(node[0]);

        if (owner is null)
        {
            return null;
        }

        if (node.Kind == SyntaxKind.Member)
        {
            return MemberPlace(owner, node);
        }

        return ElementPlaceOrNone(owner, node);
    }

    private static LValue MemberPlace(LValue owner, SyntaxNode node)
    {
        var type = owner.Type;

        if (type.Shape == TypeShape.Struct)
        {
            var (fieldType, start) = Field(type, node);

            return VariableStore.Range(owner, fieldType, start);
        }

        var components = Swizzle(type, node);

        return VariableStore.Pick(owner, HlslType.Vector(type.Kind, components.Count), components);
    }

    private LValue? ElementPlaceOrNone(LValue owner, SyntaxNode node)
    {
        if (owner.Type.Shape == TypeShape.Resource)
        {
            return null;
        }

        return ElementPlace(owner, node);
    }

    private LValue ElementPlace(LValue owner, SyntaxNode node)
    {
        var (element, length) = ElementOf(owner.Type, node);
        var index = Index(node[1]);

        if (index is Constant constant)
        {
            var position = (int)constant.Unsigned;

            if (position >= length)
            {
                throw SourceError.At(node, $"index {position} out of range");
            }

            return VariableStore.Range(owner, element, position * element.Count);
        }

        return Variables.Element(owner, element, index, length);
    }

    private static (HlslType Element, int Length) ElementOf(HlslType type, SyntaxNode node)
    {
        return type.Shape switch
        {
            TypeShape.Array => (type.Element!, type.Length),
            TypeShape.Vector => (HlslType.ScalarOf(type.Kind), type.Columns),
            TypeShape.Matrix => (HlslType.Vector(type.Kind, type.Columns), type.Rows),
            _ => throw SourceError.At(node, $"cannot index {type}"),
        };
    }

    /// <summary>A member, swizzle or constant element of a computed value.</summary>
    private RValue Part(RValue value, SyntaxNode node)
    {
        if (node.Kind == SyntaxKind.Member && value.Type.Shape == TypeShape.Struct)
        {
            var (fieldType, start) = Field(value.Type, node);

            return new RValue(fieldType, [.. value.Components.Skip(start).Take(fieldType.Count)]);
        }

        if (node.Kind == SyntaxKind.Member)
        {
            var components = Swizzle(value.Type, node);

            return new RValue(HlslType.Vector(value.Type.Kind, components.Count), [.. components.Select(component => value.Components[component])]);
        }

        var (element, length) = ElementOf(value.Type, node);
        var index = Index(node[1]);

        if (index is Constant constant && constant.Unsigned < (ulong)length)
        {
            var start = (int)constant.Unsigned * element.Count;

            return new RValue(element, [.. value.Components.Skip(start).Take(element.Count)]);
        }

        return SelectElement(value, element, length, index);
    }

    private RValue SelectElement(RValue value, HlslType element, int length, Value index)
    {
        var components = new Value[element.Count];

        for (var component = 0; component < element.Count; component++)
        {
            var result = value.Components[component];

            for (var choice = 1; choice < length; choice++)
            {
                var matches = Builder.Compare(Predicate.Equal, index, Module.Int32((uint)choice));
                result = Builder.Select(matches, value.Components[(choice * element.Count) + component], result);
            }

            components[component] = result;
        }

        return new RValue(element, components);
    }

    private static (HlslType Type, int Start) Field(HlslType structure, SyntaxNode node)
    {
        var start = 0;

        foreach (var (name, type) in structure.Fields)
        {
            if (name == node.Text)
            {
                return (type, start);
            }

            start += type.Count;
        }

        throw SourceError.At(node, $"{structure} has no member {node.Text}");
    }

    private static List<int> Swizzle(HlslType type, SyntaxNode node)
    {
        if (!type.IsNumeric || type.Shape == TypeShape.Matrix)
        {
            throw SourceError.At(node, $"cannot take .{node.Text} of {type}");
        }

        var components = new List<int>();

        foreach (var letter in node.Text)
        {
            var index = "xyzw".IndexOf(letter, StringComparison.Ordinal);

            if (index < 0)
            {
                index = "rgba".IndexOf(letter, StringComparison.Ordinal);
            }

            if (index < 0 || index >= type.Count)
            {
                throw SourceError.At(node, $"invalid swizzle .{node.Text} of {type}");
            }

            components.Add(index);
        }

        return components;
    }

    private RValue Unary(SyntaxNode node)
    {
        if (node.Text is "++" or "--")
        {
            return Increment(node[0], node.Text, post: false);
        }

        var operand = Lower(node[0]);

        if (node.Text == "+")
        {
            return operand;
        }

        return _operators.Unary(node.Text, operand);
    }

    private RValue Increment(SyntaxNode target, string op, bool post)
    {
        var place = Place(target);
        var old = Variables.Read(place);
        var one = new RValue(HlslType.LiteralInt, [Module.Int(Module.Types.Int64, 1)]);
        var updated = _operators.Binary(op[..1], old, one);
        Variables.Write(place, updated);

        if (post)
        {
            return old;
        }

        return Variables.Read(place);
    }

    private RValue Binary(SyntaxNode node)
    {
        if (node.Text is "&&" or "||")
        {
            return Logical(node);
        }

        var left = Lower(node[0]);
        var right = Lower(node[1]);

        return _operators.Binary(node.Text, left, right);
    }

    private RValue Logical(SyntaxNode node)
    {
        var left = Lower(node[0]);

        if (left.Type.Count != 1 || _purity.IsPure(node[1]))
        {
            return _operators.Binary(node.Text, left, Lower(node[1]));
        }

        var condition = _context.Convert.Convert(left, HlslType.Bool).Scalar;
        var slot = _context.Ssa.NewSlot(Module.Types.Bool);
        _context.Ssa.Write(slot, Builder.Block, condition);
        var flow = _context.Flow;
        var right = flow.NewBlock();
        var merge = flow.NewBlock();

        var start = flow.Current;

        if (node.Text == "&&")
        {
            flow.Branch(condition, right, merge);
        }
        else
        {
            flow.Branch(condition, merge, right);
        }

        _context.Ssa.BeginRegion();
        flow.Seal(right);
        flow.Enter(right);
        _context.Ssa.Write(slot, Builder.Block, Condition(node[1]));
        flow.Jump(merge);
        _context.Ssa.EndRegion(start, merge);

        flow.Seal(merge);
        flow.Enter(merge);

        return RValue.Of(HlslType.Bool, _context.Ssa.Read(slot, merge));
    }

    /// <summary>The resource an element access indexes (<c>texture[uv]</c>), or null.</summary>
    private RValue? ResourceBase(SyntaxNode node)
    {
        if (node.Kind != SyntaxKind.Index)
        {
            return null;
        }

        var owner = TryPlace(node[0]);

        if (owner is null || owner.Type.Shape != TypeShape.Resource)
        {
            return null;
        }

        return Variables.Read(owner);
    }

    private RValue Assign(SyntaxNode node)
    {
        var texture = ResourceBase(node[0]);

        if (texture is not null)
        {
            var stored = Lower(node[1]);

            if (node.Text != "=")
            {
                stored = _operators.Binary(node.Text[..^1], _context.Resources.Textures.ReadElement(node[0], texture), stored);
            }

            _context.Resources.Textures.WriteElement(node[0], texture, stored);

            return stored;
        }

        var place = Place(node[0]);
        var value = Lower(node[1]);

        if (node.Text != "=")
        {
            value = _operators.Binary(node.Text[..^1], Variables.Read(place), value);
        }

        if (place.Type.Shape == TypeShape.Resource)
        {
            value = _context.Resources.Materialize(value, place.Type);
        }

        Variables.Write(place, value);

        if (place.Variable.Storage == StorageKind.MeshOutput)
        {
            return _context.Convert.Convert(value, place.Type);
        }

        return Variables.Read(place);
    }

    private RValue Conditional(SyntaxNode node)
    {
        var condition = Lower(node[0]);

        if (condition.Type.Count > 1 || (_purity.IsPure(node[1]) && _purity.IsPure(node[FalseArm])))
        {
            return Select(condition, Lower(node[1]), Lower(node[FalseArm]));
        }

        return BranchingConditional(_context.Convert.Convert(condition, HlslType.Bool).Scalar, node);
    }

    /// <summary>Componentwise <c>select</c> after the usual conversions.</summary>
    public RValue Select(RValue condition, RValue whenTrue, RValue whenFalse)
    {
        var type = CommonType(whenTrue.Type, whenFalse.Type);
        var shape = type;

        if (type.IsNumeric && condition.Type.Count > type.Count)
        {
            shape = HlslType.Vector(type.Kind, condition.Type.Count);
        }

        var test = _context.Convert.Convert(condition, Truths(shape));
        var a = _context.Convert.Convert(whenTrue, shape);
        var b = _context.Convert.Convert(whenFalse, shape);
        var components = new Value[shape.Count];

        for (var index = 0; index < components.Length; index++)
        {
            var truth = test.Components[Math.Min(index, test.Components.Count - 1)];
            components[index] = Builder.Select(truth, a.Components[index], b.Components[index]);
        }

        return new RValue(shape, components);
    }

    private static HlslType Truths(HlslType shape)
    {
        if (shape.IsNumeric)
        {
            return shape.WithKind(ScalarKind.Bool);
        }

        return HlslType.Bool;
    }

    private static HlslType CommonType(HlslType a, HlslType b)
    {
        if (!a.IsNumeric || !b.IsNumeric)
        {
            return a;
        }

        var kind = Conversions.Common(a.Kind, b.Kind);

        if (kind is ScalarKind.LiteralInt)
        {
            kind = ScalarKind.Int;
        }

        if (kind is ScalarKind.LiteralFloat)
        {
            kind = ScalarKind.Float;
        }

        return Conversions.CommonShape(a, b, kind);
    }

    private RValue BranchingConditional(Value condition, SyntaxNode node)
    {
        var flow = _context.Flow;
        var (whenTrue, whenFalse, merge) = (flow.NewBlock(), flow.NewBlock(), flow.NewBlock());
        var start = flow.Current;
        flow.Branch(condition, whenTrue, whenFalse);
        _context.Ssa.BeginRegion();
        flow.Seal(whenTrue);

        flow.Seal(whenFalse);
        flow.Enter(whenTrue);
        var first = Lower(node[1]);
        var firstEnd = flow.Current;
        flow.Enter(whenFalse);
        var second = Lower(node[FalseArm]);

        var secondEnd = flow.Current;
        var type = CommonType(first.Type, second.Type);
        var slots = Variables.DeclareSlots("?:", type);
        var place = Variables.Whole(slots);

        foreach (var (end, value) in new[] { (firstEnd, first), (secondEnd, second) })
        {
            flow.Enter(end);
            Variables.Write(place, value);
            flow.Jump(merge);
        }

        _context.Ssa.EndRegion(start, merge);
        flow.Seal(merge);
        flow.Enter(merge);

        return Variables.Read(place);
    }

    private RValue Call(SyntaxNode node)
    {
        if (_context.Functions.TryGetValue(node.Text, out var overloads))
        {
            return new FunctionInliner(_context).Call(node, Overload(node, overloads));
        }

        if (_context.Types.IsStruct(node.Text) || TypeNames.IsScalar(node.Text) || TypeNames.Shape(node.Text) is not null)
        {
            return Construct(node);
        }

        return _context.Intrinsics.Call(node);
    }

    private static SyntaxNode Overload(SyntaxNode call, List<SyntaxNode> overloads)
    {
        foreach (var function in overloads)
        {
            var parameters = 0;

            foreach (var child in function.Children)
            {
                if (child.Kind == SyntaxKind.Parameter)
                {
                    parameters++;
                }
            }

            if (parameters == call.Children.Count)
            {
                return function;
            }
        }

        throw SourceError.At(call, $"no overload of {call.Text} takes {call.Children.Count} arguments");
    }

    private RValue Construct(SyntaxNode node)
    {
        var type = _context.Types.Base(new TypeSyntax(node.Text));
        var arguments = node.Children.Select(argument => Lower(argument)).ToList();

        if (arguments.Count == 1 && arguments[0].Type.Count == 1)
        {
            return _context.Convert.Convert(arguments[0], type);
        }

        var components = new List<Value>();

        foreach (var argument in arguments)
        {
            for (var index = 0; index < argument.Type.Count; index++)
            {
                components.Add(_context.Convert.Scalar(argument.Components[index], argument.Type.ComponentKind(index), type.ComponentKind(components.Count)));
            }
        }

        if (components.Count != type.Count)
        {
            throw SourceError.At(node, $"{node.Text} needs {type.Count} components, got {components.Count}");
        }

        return new RValue(type, components);
    }

    private RValue Cast(SyntaxNode node)
    {
        var type = _context.Types.Resolve(node.Type!, dimension => (int)ConstantInteger(dimension!));
        var value = Lower(node[0]);

        if (type.IsNumeric && value.Type.Shape == TypeShape.Vector && value.Type.Count > type.Count)
        {
            value = new RValue(HlslType.Vector(value.Type.Kind, type.Count), [.. value.Components.Take(type.Count)]);
        }

        return _context.Convert.Convert(value, type);
    }

    /// <summary>A braced list: its leaves' components in order, typed as the caller converts them.</summary>
    private RValue InitializerList(SyntaxNode node)
    {
        var components = new List<Value>();
        var kinds = new List<ScalarKind>();
        Flatten(node, components, kinds);
        var kind = kinds.Aggregate(Conversions.Common);
        var converted = components.Select((component, index) => _context.Convert.Scalar(component, kinds[index], kind)).ToList();

        return new RValue(HlslType.Array(HlslType.ScalarOf(kind), converted.Count), converted);
    }

    private void Flatten(SyntaxNode node, List<Value> components, List<ScalarKind> kinds)
    {
        if (node.Kind == SyntaxKind.InitializerList)
        {
            foreach (var child in node.Children)
            {
                Flatten(child, components, kinds);
            }

            return;
        }

        var value = Lower(node);

        for (var index = 0; index < value.Type.Count; index++)
        {
            components.Add(value.Components[index]);
            kinds.Add(value.Type.ComponentKind(index));
        }
    }

    private RValue Comma(SyntaxNode node)
    {
        var result = RValue.Void;

        foreach (var child in node.Children)
        {
            result = Lower(child);
        }

        return result;
    }
}
