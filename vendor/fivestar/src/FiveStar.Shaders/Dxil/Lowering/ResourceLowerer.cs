// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Ops;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Resources: <c>ResourceDescriptorHeap[i]</c> and <c>SamplerDescriptorHeap[i]</c> become annotated heap handles when
/// they meet a resource type, the user-data constant buffer gets its bound handle, and resource methods become DXIL
/// operations (raw buffers here, textures in <see cref="TextureLowerer"/>).
/// </summary>
public sealed class ResourceLowerer
{
    private const string ResourceHeap = "ResourceDescriptorHeap";
    private const string SamplerHeap = "SamplerDescriptorHeap";

    private readonly LoweringContext _context;
    private readonly HashSet<Value> _nonUniform = [];
    private readonly Dictionary<(Value Index, string Type), Value> _handles = [];
    private TextureLowerer? _textures;

    public ResourceLowerer(LoweringContext context)
    {
        _context = context;
    }

    private IrBuilder Builder => _context.Builder;

    private DxilModule Module => _context.Module;

    public TextureLowerer Textures => _textures ??= new TextureLowerer(_context);

    public bool IsHeap(string name)
    {
        return name is ResourceHeap or SamplerHeap && _context.Scope.Find(name) is null;
    }

    /// <summary>A heap element before it has a type: the index, typed as the heap.</summary>
    public static RValue HeapReference(string heap, Value index)
    {
        return RValue.Of(HlslType.Resource(heap, null), index);
    }

    /// <summary>Marks a value as varying between lanes when used as a heap index (NonUniformResourceIndex).</summary>
    public void MarkNonUniform(Value value)
    {
        _nonUniform.Add(value);
    }

    /// <summary>A value of resource type <paramref name="type"/>: a heap reference becomes its annotated handle.</summary>
    public RValue Materialize(RValue value, HlslType type)
    {
        if (value.Type.Name is not (ResourceHeap or SamplerHeap))
        {
            return value;
        }

        var index = value.Scalar;
        var sampler = value.Type.Name == SamplerHeap;
        var key = (index, type.Name + type.Element);

        if (_handles.TryGetValue(key, out var cached) && Dominates(cached))
        {
            return RValue.Of(type, cached);
        }

        var (kind, detail) = ResourceProperties.Of(type, Module);
        var handle = _context.Ops.Call(
            DxOpCode.CreateHandleFromHeap, "createHandleFromHeap", AttributeSet.ReadNone, _context.Ops.Handle,
            index, Module.Bool(sampler), Module.Bool(_nonUniform.Contains(index)));
        var annotated = _context.Ops.Annotate(handle, kind, detail);
        _handles[key] = annotated;

        return RValue.Of(type, annotated);
    }

    /// <summary>The handle of the user-data constant buffer, made in the entry block.</summary>
    public Value UserData()
    {
        if (_context.UserDataHandle is null)
        {
            throw new DxilBackendException("the program reads a constant buffer that is not declared");
        }

        return _context.UserDataHandle;
    }

    /// <summary>A method call on a resource.</summary>
    public RValue Method(SyntaxNode call)
    {
        var target = _context.Expressions.Lower(call[0]);

        if (target.Type.Shape != TypeShape.Resource)
        {
            throw SourceError.At(call, $"{call.Text} on {target.Type}");
        }

        var name = target.Type.Name;

        if (name.EndsWith("ByteAddressBuffer", StringComparison.Ordinal))
        {
            return new RawBufferLowerer(_context).Method(call, target.Scalar);
        }

        return Textures.Method(call, target);
    }

    /// <summary>
    /// A handle made earlier is reused only from a block it dominates; blocks are made in order, so the cache is
    /// trusted only within the block that made it.
    /// </summary>
    private bool Dominates(Value handle)
    {
        return handle is Instruction instruction && ReferenceEquals(instruction.Block, Builder.Block);
    }
}
