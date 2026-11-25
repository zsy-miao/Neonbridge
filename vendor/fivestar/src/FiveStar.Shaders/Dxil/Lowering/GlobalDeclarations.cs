// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Top-level declarations: structs, the user-data constant buffer at b0, functions, and global variables (constants
/// and statics as slots or tables, shared memory as address-space-3 arrays), initialized in the entry block in order.
/// </summary>
public sealed class GlobalDeclarations
{
    private const int RowBytes = 16;

    private readonly LoweringContext _context;

    public GlobalDeclarations(LoweringContext context)
    {
        _context = context;
    }

    /// <summary>Records structs and functions (types first, so later declarations can name them).</summary>
    public void Collect(IReadOnlyList<SyntaxNode> declarations)
    {
        foreach (var declaration in declarations)
        {
            if (declaration.Kind == SyntaxKind.Struct)
            {
                Struct(declaration);
            }
            else if (declaration.Kind == SyntaxKind.Function && declaration.Children.Any(child => child.Kind == SyntaxKind.Block))
            {
                if (!_context.Functions.TryGetValue(declaration.Text, out var overloads))
                {
                    overloads = [];
                    _context.Functions[declaration.Text] = overloads;
                }

                overloads.Add(declaration);
            }
        }
    }

    /// <summary>Declares the constant buffer and the globals, in source order, in the entry block.</summary>
    public void Declare(IReadOnlyList<SyntaxNode> declarations)
    {
        foreach (var declaration in declarations)
        {
            if (declaration.Kind == SyntaxKind.Cbuffer)
            {
                Cbuffer(declaration);
            }
            else if (declaration.Kind == SyntaxKind.Variable)
            {
                Variable(declaration);
            }
        }
    }

    private void Struct(SyntaxNode declaration)
    {
        var fields = declaration.Children
            .Select(field => (field.Text, _context.Types.Resolve(field.Type!, size => (int)_context.Expressions.ConstantInteger(size!))))
            .ToList();
        _context.Types.AddStruct(declaration.Text, HlslType.Struct(declaration.Text, fields));
        _context.StructDeclarations[declaration.Text] = declaration;
    }

    private void Cbuffer(SyntaxNode declaration)
    {
        if (declaration.Semantic is not null && declaration.Semantic != "register(b0)")
        {
            throw SourceError.At(declaration, $"constant buffer at {declaration.Semantic}: only b0 is supported");
        }

        var offset = 0;

        foreach (var field in declaration.Children)
        {
            var type = _context.Types.Resolve(field.Type!, size => (int)_context.Expressions.ConstantInteger(size!));

            if (type.Shape == TypeShape.Array && type.Element!.Count * sizeof(uint) != RowBytes)
            {
                throw SourceError.At(field, "constant-buffer arrays of elements other than 16 bytes are not supported");
            }

            offset = Align(offset, type);
            _context.Scope.DeclareGlobal(VariableStore.DeclareCbuffer(field.Text, type, offset));
            offset += type.Count * sizeof(uint);
        }

        _context.UserDataBytes = (offset + RowBytes - 1) / RowBytes * RowBytes;
        _context.UserDataHandle = _context.Ops.UserDataHandle((uint)_context.UserDataBytes);
    }

    /// <summary>HLSL packing: an array or a value that would cross a 16-byte row starts a new row.</summary>
    private static int Align(int offset, HlslType type)
    {
        var size = type.Count * sizeof(uint);
        var rowStart = offset / RowBytes * RowBytes;

        if (type.Shape == TypeShape.Array || offset + size > rowStart + RowBytes)
        {
            return (offset + RowBytes - 1) / RowBytes * RowBytes;
        }

        return offset;
    }

    private void Variable(SyntaxNode declaration)
    {
        var modifiers = declaration.Modifiers;

        foreach (var declarator in declaration.Children)
        {
            var type = _context.Types.Resolve(declarator.Type!, size => _context.Statements.ArraySize(size, declarator));
            var initializer = declarator.Children.FirstOrDefault();

            if (modifiers.Contains("groupshared"))
            {
                _context.Scope.DeclareGlobal(_context.Variables.DeclareMemory(declarator.Text, type, shared: true, table: null));

                continue;
            }

            if (!modifiers.Contains("static") && !modifiers.Contains("const"))
            {
                throw SourceError.At(declarator, $"global {declarator.Text}: bound resources and uniforms are not supported");
            }

            Static(declarator, type, initializer, modifiers.Contains("const"));
        }
    }

    private void Static(SyntaxNode declarator, HlslType type, SyntaxNode? initializer, bool constant)
    {
        if (type.Shape == TypeShape.Array && constant && initializer is not null)
        {
            var table = _context.Expressions.Lower(initializer);

            if (table.Components.All(component => component is Constant))
            {
                var converted = _context.Convert.Convert(table, type);
                _context.Scope.DeclareGlobal(_context.Variables.DeclareMemory(declarator.Text, type, shared: false, converted.Components));

                return;
            }
        }

        Variable variable;

        if (type.Shape == TypeShape.Array || (_context.MemoryLocals && type.IsNumeric && !constant))
        {
            variable = _context.Variables.DeclareMemory(declarator.Text, type, shared: false, table: null);
        }
        else
        {
            variable = _context.Variables.DeclareSlots(declarator.Text, type);
        }

        _context.Scope.DeclareGlobal(variable);

        if (initializer is not null)
        {
            _context.Statements.Initialize(variable, type, initializer);
        }

        if (constant && initializer is not null)
        {
            _context.Statements.Fix(variable);
        }
        else if (variable.Storage == StorageKind.Slots && type.Shape != TypeShape.Resource)
        {
            var zero = RValue.Of(HlslType.LiteralInt, _context.Module.Int(_context.Module.Types.Int64, 0));
            _context.Variables.Write(_context.Variables.Whole(variable), zero);
        }
    }
}
