// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Inlines a user function at its call (DXIL has one function): arguments are evaluated in the caller, parameters
/// become variables of a fresh scope, <c>return</c> jumps to the call's exit with the value in slots, and
/// <c>out</c>/<c>inout</c> parameters are copied back at the exit.
/// </summary>
public sealed class FunctionInliner
{
    private const int MaxDepth = 64;

    private readonly LoweringContext _context;

    public FunctionInliner(LoweringContext context)
    {
        _context = context;
    }

    public RValue Call(SyntaxNode call, SyntaxNode function)
    {
        var parameters = function.Children.Where(child => child.Kind == SyntaxKind.Parameter).ToList();
        var body = function.Children.LastOrDefault(child => child.Kind == SyntaxKind.Block)
            ?? throw SourceError.At(call, $"{function.Text} has no body");
        var arguments = Arguments(call, parameters);

        if (_context.CallDepth >= MaxDepth)
        {
            throw SourceError.At(call, $"calls nested too deeply at {function.Text} (recursion?)");
        }

        _context.CallDepth++;

        try
        {
            return Inline(function, parameters, body, arguments);
        }
        finally
        {
            _context.CallDepth--;
        }
    }

    private List<(RValue? Value, LValue? Target)> Arguments(SyntaxNode call, List<SyntaxNode> parameters)
    {
        var arguments = new List<(RValue? Value, LValue? Target)>();

        for (var index = 0; index < parameters.Count; index++)
        {
            var parameter = parameters[index];
            var copiesOut = parameter.HasModifier("out") || parameter.HasModifier("inout");
            var copiesIn = !parameter.HasModifier("out");
            var argument = call.Children[index];
            LValue? target = null;
            RValue? value = null;

            if (copiesOut)
            {
                target = _context.Expressions.Place(argument);
            }

            if (copiesIn && target is not null)
            {
                value = _context.Variables.Read(target);
            }
            else if (copiesIn)
            {
                value = _context.Expressions.Lower(argument);
            }

            arguments.Add((value, target));
        }

        return arguments;
    }

    private RValue Inline(SyntaxNode function, List<SyntaxNode> parameters, SyntaxNode body, List<(RValue? Value, LValue? Target)> arguments)
    {
        var saved = _context.Scope.Isolate();
        var loops = _context.Flow.SaveLoops();
        _context.Scope.Push();
        var parameterVariables = Bind(parameters, arguments);
        var returnType = _context.Types.Resolve(function.Type!, _ => throw SourceError.At(function, "an array return type is not supported"));
        var returnVariable = _context.Variables.DeclareSlots(function.Text + ".return", returnType);

        var returnCount = returnType.Shape == TypeShape.Void ? 0 : returnType.Count;
        var returnSlots = Enumerable.Range(returnVariable.FirstSlot, returnCount).ToList();
        var exit = _context.Flow.NewBlock();
        var start = _context.Flow.Current;
        _context.Ssa.BeginRegion();
        _context.Flow.PushFrame(new FunctionFrame(exit, returnType, returnSlots));

        _context.Statements.LowerBlockContents(body);
        _context.Flow.Jump(exit);
        _context.Flow.PopFrame();
        _context.Ssa.EndRegion(start, exit);
        _context.Flow.Seal(exit);
        _context.Flow.Enter(exit);

        var result = RValue.Void;

        if (returnType.Shape != TypeShape.Void)
        {
            result = _context.Variables.Read(_context.Variables.Whole(returnVariable));
        }

        var outputs = parameterVariables.Select(variable => _context.Variables.Read(_context.Variables.Whole(variable))).ToList();
        _context.Scope.Pop();
        _context.Scope.Restore(saved);
        _context.Flow.RestoreLoops(loops);
        CopyOut(arguments, outputs);

        return result;
    }

    private List<Variable> Bind(List<SyntaxNode> parameters, List<(RValue? Value, LValue? Target)> arguments)
    {
        var variables = new List<Variable>();

        for (var index = 0; index < parameters.Count; index++)
        {
            var parameter = parameters[index];
            var type = _context.Types.Resolve(parameter.Type!, dimension => (int)_context.Expressions.ConstantInteger(dimension!));
            var variable = _context.Statements.DeclareLocal(parameter.Text, type);
            var value = arguments[index].Value;

            if (value is not null)
            {
                if (type.Shape == TypeShape.Resource)
                {
                    value = _context.Resources.Materialize(value, type);
                }

                _context.Variables.Write(_context.Variables.Whole(variable), value);
            }

            variables.Add(variable);
        }

        return variables;
    }

    private void CopyOut(List<(RValue? Value, LValue? Target)> arguments, List<RValue> outputs)
    {
        for (var index = 0; index < arguments.Count; index++)
        {
            var target = arguments[index].Target;

            if (target is not null)
            {
                _context.Variables.Write(target, outputs[index]);
            }
        }
    }
}
