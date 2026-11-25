// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Lowers statements: local declarations, structured control flow (if, for, while, do, switch with fall-through) into
/// blocks, and break, continue and return as jumps. A block is sealed for SSA once every branch into it exists.
/// </summary>
public sealed class StatementLowerer
{
    private const int ElseBranch = 2;
    private const int ForStep = 2;
    private const int ForBody = 3;

    private readonly LoweringContext _context;

    public StatementLowerer(LoweringContext context)
    {
        _context = context;
    }

    private ControlFlow Flow => _context.Flow;

    private ExpressionLowerer Expressions => _context.Expressions;

    /// <summary>The statements of a block in a new scope.</summary>
    public void LowerBlockContents(SyntaxNode block)
    {
        _context.Scope.Push();

        foreach (var statement in block.Children)
        {
            Lower(statement);
        }

        _context.Scope.Pop();
    }

    /// <summary>A local variable: arrays in memory (numeric ones too with <see cref="LoweringContext.MemoryLocals"/>), everything else in slots.</summary>
    public Variable DeclareLocal(string name, HlslType type)
    {
        Variable variable;

        if (type.Shape == TypeShape.Array || (_context.MemoryLocals && type.IsNumeric))
        {
            variable = _context.Variables.DeclareMemory(name, type, shared: false, table: null);
        }
        else
        {
            variable = _context.Variables.DeclareSlots(name, type);
        }

        _context.Scope.Declare(variable);

        return variable;
    }

    public void Lower(SyntaxNode statement)
    {
        switch (statement.Kind)
        {
            case SyntaxKind.Block:
                LowerBlockContents(statement);
                break;
            case SyntaxKind.Variable:
                Declaration(statement);
                break;
            case SyntaxKind.ExpressionStatement:
                Expressions.Lower(statement[0]);
                break;
            case SyntaxKind.If:
                If(statement);
                break;
            case SyntaxKind.Empty:
                break;
            default:
                LowerJumpOrLoop(statement);
                break;
        }
    }

    private void LowerJumpOrLoop(SyntaxNode statement)
    {
        switch (statement.Kind)
        {
            case SyntaxKind.For:
                For(statement);
                break;
            case SyntaxKind.While:
                While(statement);
                break;
            case SyntaxKind.Do:
                Do(statement);
                break;
            case SyntaxKind.Switch:
                Switch(statement);
                break;
            case SyntaxKind.Break:
                Flow.Break();
                break;
            case SyntaxKind.Continue:
                Flow.Continue();
                break;
            case SyntaxKind.Return:
                Return(statement);
                break;
            case SyntaxKind.Discard:
                _context.Intrinsics.Discard();
                break;
            default:
                throw SourceError.At(statement, $"{statement.Kind} is not a statement");
        }
    }

    private void Declaration(SyntaxNode declaration)
    {
        foreach (var declarator in declaration.Children)
        {
            var type = _context.Types.Resolve(declarator.Type!, dimension => ArraySize(dimension, declarator));
            var initializer = declarator.Children.FirstOrDefault();

            if (type.Shape == TypeShape.Array && declaration.HasModifier("const") && initializer is not null)
            {
                var table = Expressions.Lower(initializer);

                if (table.Components.All(component => component is Constant))
                {
                    var converted = _context.Convert.Convert(table, type);
                    _context.Scope.Declare(_context.Variables.DeclareMemory(declarator.Text, type, shared: false, converted.Components));

                    continue;
                }
            }

            var variable = DeclareLocal(declarator.Text, type);

            if (initializer is not null)
            {
                Initialize(variable, type, initializer);
            }

            if (declaration.HasModifier("const") && initializer is not null)
            {
                Fix(variable);
            }
        }
    }

    /// <summary>A const variable whose value is constant: reads take the constant (also where a label needs one).</summary>
    public void Fix(Variable variable)
    {
        if (variable.Storage != StorageKind.Slots || variable.Type.Shape == TypeShape.Resource)
        {
            return;
        }

        var value = _context.Variables.Read(_context.Variables.Whole(variable));

        if (value.Components.All(component => component is Constant))
        {
            variable.Fixed = value.Components;
        }
    }

    /// <summary>Stores a declaration's initial value (a resource from the heap becomes a handle).</summary>
    public void Initialize(Variable variable, HlslType type, SyntaxNode initializer)
    {
        var value = Expressions.Lower(initializer);

        if (type.Shape == TypeShape.Resource)
        {
            value = _context.Resources.Materialize(value, type);
        }

        _context.Variables.Write(_context.Variables.Whole(variable), value);
    }

    /// <summary>An array dimension; an unsized one takes the initializer's length.</summary>
    public int ArraySize(SyntaxNode? dimension, SyntaxNode declarator)
    {
        if (dimension is not null)
        {
            return (int)Expressions.ConstantInteger(dimension);
        }

        var initializer = declarator.Children.FirstOrDefault();

        if (initializer is null || initializer.Kind != SyntaxKind.InitializerList)
        {
            throw SourceError.At(declarator, "an unsized array needs an initializer list");
        }

        return initializer.Children.Count;
    }

    private void If(SyntaxNode statement)
    {
        var condition = Expressions.Condition(statement[0]);
        var whenTrue = Flow.NewBlock();
        var merge = Flow.NewBlock();
        var whenFalse = merge;

        if (statement.Children.Count > ElseBranch)
        {
            whenFalse = Flow.NewBlock();
        }

        var start = Flow.Current;
        Flow.Branch(condition, whenTrue, whenFalse);
        _context.Ssa.BeginRegion();
        Flow.Seal(whenTrue);
        Flow.Enter(whenTrue);
        Lower(statement[1]);

        Flow.Jump(merge);

        if (statement.Children.Count > ElseBranch)
        {
            Flow.Seal(whenFalse);
            Flow.Enter(whenFalse);
            Lower(statement[ElseBranch]);
            Flow.Jump(merge);
        }

        _context.Ssa.EndRegion(start, merge);
        Flow.Seal(merge);
        Flow.Enter(merge);
    }

    private void For(SyntaxNode statement)
    {
        _context.Scope.Push();
        Lower(statement[0]);
        SyntaxNode? step = statement[ForStep];

        if (step.Kind == SyntaxKind.Empty)
        {
            step = null;
        }

        Loop(statement[1], statement[ForBody], step);
        _context.Scope.Pop();
    }

    private void While(SyntaxNode statement)
    {
        Loop(statement[0], statement[1], null);
    }

    /// <summary>
    /// A loop that tests <paramref name="condition"/> (Empty: none) before each pass, runs <paramref name="body"/>, then
    /// <paramref name="step"/>: header (condition), body, step block (the continue target), exit.
    /// </summary>
    private void Loop(SyntaxNode condition, SyntaxNode body, SyntaxNode? step)
    {
        var header = Flow.NewBlock();
        var bodyBlock = Flow.NewBlock();
        var next = Flow.NewBlock();
        var exit = Flow.NewBlock();
        Flow.Jump(header);
        Flow.Enter(header);

        _context.Ssa.BeginRegion();

        if (condition.Kind == SyntaxKind.Empty)
        {
            Flow.Jump(bodyBlock);
        }
        else
        {
            Flow.Branch(Expressions.Condition(condition), bodyBlock, exit);
        }

        Flow.Seal(bodyBlock);
        Flow.Enter(bodyBlock);
        Flow.PushLoop(exit, next);
        Lower(body);
        Flow.PopLoop();
        Flow.Jump(next);

        Flow.Seal(next);
        Flow.Enter(next);

        if (step is not null)
        {
            Expressions.Lower(step);
        }

        Flow.Jump(header);
        Flow.Seal(header);
        _context.Ssa.EndRegion(header, exit);
        Flow.Seal(exit);
        Flow.Enter(exit);
    }

    private void Do(SyntaxNode statement)
    {
        var body = Flow.NewBlock();
        var test = Flow.NewBlock();
        var exit = Flow.NewBlock();
        Flow.Jump(body);
        Flow.Enter(body);
        _context.Ssa.BeginRegion();

        Flow.PushLoop(exit, test);
        Lower(statement[0]);
        Flow.PopLoop();
        Flow.Jump(test);
        Flow.Seal(test);
        Flow.Enter(test);

        Flow.Branch(Expressions.Condition(statement[1]), body, exit);
        Flow.Seal(body);
        _context.Ssa.EndRegion(body, exit);
        Flow.Seal(exit);
        Flow.Enter(exit);
    }

    private void Switch(SyntaxNode statement)
    {
        var value = Expressions.Lower(statement[0]);
        var kind = value.Type.Kind == ScalarKind.Int ? ScalarKind.Int : ScalarKind.Uint;
        var selector = _context.Convert.Convert(value, HlslType.ScalarOf(kind)).Scalar;
        var cases = statement.Children.Skip(1).ToList();
        var blocks = cases.Select(_ => Flow.NewBlock()).ToList();
        var exit = Flow.NewBlock();

        var fallback = exit;
        var labels = new List<(Constant Value, BasicBlock Target)>();

        for (var index = 0; index < cases.Count; index++)
        {
            foreach (var label in cases[index].Children)
            {
                if (label.Kind == SyntaxKind.Name && label.Text.Length == 0)
                {
                    fallback = blocks[index];

                    continue;
                }

                var number = _context.Module.Int32((uint)Expressions.ConstantInteger(label));
                labels.Add((number, blocks[index]));
            }
        }

        var start = Flow.Current;
        Flow.Switch(selector, fallback, labels);
        _context.Ssa.BeginRegion();
        Flow.PushLoop(exit, null);
        Cases(cases, blocks, exit);
        Flow.PopLoop();

        _context.Ssa.EndRegion(start, exit);
        Flow.Seal(exit);
        Flow.Enter(exit);
    }

    private void Cases(List<SyntaxNode> cases, List<BasicBlock> blocks, BasicBlock exit)
    {
        for (var index = 0; index < cases.Count; index++)
        {
            Flow.Seal(blocks[index]);
            Flow.Enter(blocks[index]);

            foreach (var statement in cases[index].Body)
            {
                Lower(statement);
            }

            var next = exit;

            if (index + 1 < cases.Count)
            {
                next = blocks[index + 1];
            }

            Flow.Jump(next);
        }
    }

    private void Return(SyntaxNode statement)
    {
        var frame = Flow.Frame;

        if (statement.Children.Count > 0)
        {
            var value = Expressions.Lower(statement[0], frame.ReturnType);

            for (var index = 0; index < frame.ReturnSlots.Count; index++)
            {
                _context.Ssa.Write(frame.ReturnSlots[index], _context.Builder.Block, value.Components[index]);
            }
        }

        Flow.Jump(frame.Exit);
        Flow.Unreachable();
    }
}
