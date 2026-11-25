// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Variables and the places inside them: declares slot, memory and constant-buffer variables, narrows a place to a
/// member, element or swizzle, and reads and writes places component by component.
/// </summary>
public sealed class VariableStore
{
    private const int DwordsPerRow = 4;
    private const int RowShift = 2;
    private const int SharedSpace = 3;

    private readonly LoweringContext _context;

    public VariableStore(LoweringContext context)
    {
        _context = context;
    }

    private IrBuilder Builder => _context.Builder;

    private DxilModule Module => _context.Module;

    /// <summary>A variable held in SSA slots (one per component; a resource's handle in one).</summary>
    public Variable DeclareSlots(string name, HlslType type)
    {
        var first = -1;

        for (var index = 0; index < type.Count; index++)
        {
            var slot = _context.Ssa.NewSlot(SlotType(type, index));

            if (index == 0)
            {
                first = slot;
            }
        }

        return new Variable(name, type, StorageKind.Slots) { FirstSlot = first };
    }

    /// <summary>
    /// An array in memory: thread-group shared (<paramref name="shared"/>), a constant table (<paramref name="table"/>
    /// holds its components) or a local array.
    /// </summary>
    public Variable DeclareMemory(string name, HlslType type, bool shared, IReadOnlyList<Value>? table)
    {
        var element = MemoryElement(type);
        var arrayType = Module.Types.Array(type.Count, element);

        if (shared)
        {
            _context.SharedBytes += type.Count * (element.Bits / sizeof(long));
            var global = Module.Global(name, arrayType, SharedSpace, null);

            return new Variable(name, type, StorageKind.Memory) { Pointer = global, ElementType = element };
        }

        if (table is not null)
        {
            var constants = new Constant[type.Count];

            for (var index = 0; index < type.Count; index++)
            {
                constants[index] = (Constant)ToMemory(table[index], type.ComponentKind(index), element);
            }

            var global = Module.Global(name, arrayType, 0, Module.Aggregate(arrayType, constants));

            return new Variable(name, type, StorageKind.Memory) { Pointer = global, ElementType = element, IsConstant = true };
        }

        var alloca = Builder.Alloca(_context.EntryBlock!, arrayType);

        return new Variable(name, type, StorageKind.Memory) { Pointer = alloca, ElementType = element };
    }

    public static Variable DeclareCbuffer(string name, HlslType type, int offset)
    {
        return new Variable(name, type, StorageKind.Cbuffer) { CbufferOffset = offset };
    }

    /// <summary>The whole variable as a place.</summary>
    public LValue Whole(Variable variable)
    {
        variable.WholePlace ??= MakeWhole(variable);

        return variable.WholePlace;
    }

    private LValue MakeWhole(Variable variable)
    {
        var count = variable.Type.Count;

        return variable.Storage switch
        {
            StorageKind.Slots => new LValue(variable.Type, variable) { Slots = [.. Enumerable.Range(variable.FirstSlot, count)] },
            StorageKind.Cbuffer => new LValue(variable.Type, variable) { Indices = Counting(variable.CbufferOffset / sizeof(uint), count) },
            StorageKind.MeshOutput or StorageKind.GeometryInput => new LValue(variable.Type, variable) { Indices = Counting(0, count) },
            _ => new LValue(variable.Type, variable) { Indices = Counting(0, count) },
        };
    }

    /// <summary>Components <paramref name="start"/> .. start + type.Count - 1 (a member or a constant element).</summary>
    public static LValue Range(LValue place, HlslType type, int start)
    {
        return Pick(place, type, [.. Enumerable.Range(start, type.Count)]);
    }

    /// <summary>The listed components of a place (a swizzle).</summary>
    public static LValue Pick(LValue place, HlslType type, IReadOnlyList<int> components)
    {
        if (place.IsDynamic)
        {
            var choices = place.Choices.Select(choice => (IReadOnlyList<int>)[.. components.Select(component => choice[component])]).ToList();

            return new LValue(type, place.Variable) { Choices = choices, Selector = place.Selector };
        }

        if (place.Variable.Storage == StorageKind.Slots)
        {
            return new LValue(type, place.Variable) { Slots = [.. components.Select(component => place.Slots[component])] };
        }

        return new LValue(type, place.Variable) { Indices = [.. components.Select(component => place.Indices[component])], ArrayIndex = place.ArrayIndex };
    }

    /// <summary>Element <paramref name="index"/> (not a constant) of an array, vector or matrix place.</summary>
    public LValue Element(LValue place, HlslType element, Value index, int length)
    {
        if (place.Variable.Storage is StorageKind.MeshOutput or StorageKind.GeometryInput)
        {
            return new LValue(element, place.Variable) { Indices = Counting(0, element.Count), ArrayIndex = index };
        }

        if (place.Variable.Storage != StorageKind.Slots)
        {
            var scaled = Builder.Binary(BinaryOp.Mul, index, Module.Int32((uint)element.Count));
            var indices = new Value[element.Count];

            for (var component = 0; component < element.Count; component++)
            {
                var offset = Builder.Binary(BinaryOp.Add, place.Indices[0], Module.Int32((uint)component));
                indices[component] = Builder.Binary(BinaryOp.Add, scaled, offset);
            }

            return new LValue(element, place.Variable) { Indices = indices };
        }

        if (place.IsDynamic)
        {
            throw new DxilBackendException("a dynamic index into a dynamically indexed value is not supported");
        }

        var choices = Enumerable.Range(0, length)
            .Select(choice => (IReadOnlyList<int>)[.. place.Slots.Skip(choice * element.Count).Take(element.Count)])
            .ToList();

        return new LValue(element, place.Variable) { Choices = choices, Selector = index };
    }

    public RValue Read(LValue place)
    {
        if (place.Variable.Storage == StorageKind.MeshOutput)
        {
            throw new DxilBackendException($"{place.Variable.Name} is a mesh output (write only)");
        }

        if (place.Variable.Storage == StorageKind.GeometryInput)
        {
            return _context.Geometry!.Read(place);
        }

        var components = new Value[place.Type.Count];

        for (var index = 0; index < components.Length; index++)
        {
            components[index] = ReadComponent(place, index);
        }

        return new RValue(place.Type, components);
    }

    public void Write(LValue place, RValue value)
    {
        if (place.Variable.IsConstant || place.Variable.Storage is StorageKind.Cbuffer or StorageKind.GeometryInput)
        {
            throw new DxilBackendException($"{place.Variable.Name} is read-only");
        }

        var converted = _context.Convert.Convert(value, place.Type);

        if (place.Variable.Storage == StorageKind.MeshOutput)
        {
            _context.Mesh!.Write(place, converted);

            return;
        }

        for (var index = 0; index < place.Type.Count; index++)
        {
            WriteComponent(place, index, converted.Components[index]);
        }
    }

    /// <summary>The pointer to component <paramref name="index"/> of a memory place (atomics).</summary>
    public Value Address(LValue place, int index)
    {
        return Builder.Element(place.Variable.Pointer!, place.Indices[index]);
    }

    private Value ReadComponent(LValue place, int index)
    {
        var kind = place.Type.ComponentKind(index);

        if (place.IsDynamic)
        {
            var value = _context.Ssa.Read(place.Choices[0][index], Builder.Block);

            for (var choice = 1; choice < place.Choices.Count; choice++)
            {
                var other = _context.Ssa.Read(place.Choices[choice][index], Builder.Block);
                var matches = Builder.Compare(Predicate.Equal, place.Selector!, Module.Int32((uint)choice));
                value = Builder.Select(matches, other, value);
            }

            return value;
        }

        var fixedValues = place.Variable.Fixed;

        if (fixedValues is not null && place.Variable.Storage == StorageKind.Slots)
        {
            return fixedValues[place.Slots[index] - place.Variable.FirstSlot];
        }

        return place.Variable.Storage switch
        {
            StorageKind.Slots => _context.Ssa.Read(place.Slots[index], Builder.Block),
            StorageKind.Cbuffer => FromMemory(CbufferDword(place.Indices[index]), kind),
            _ => FromMemory(Builder.Load(Address(place, index)), kind),
        };
    }

    private void WriteComponent(LValue place, int index, Value value)
    {
        if (place.IsDynamic)
        {
            for (var choice = 0; choice < place.Choices.Count; choice++)
            {
                var slot = place.Choices[choice][index];
                var old = _context.Ssa.Read(slot, Builder.Block);
                var matches = Builder.Compare(Predicate.Equal, place.Selector!, Module.Int32((uint)choice));
                _context.Ssa.Write(slot, Builder.Block, Builder.Select(matches, value, old));
            }

            return;
        }

        if (place.Variable.Storage == StorageKind.Slots)
        {
            _context.Ssa.Write(place.Slots[index], Builder.Block, value);

            return;
        }

        var element = place.Variable.ElementType!;
        Builder.Store(Address(place, index), ToMemory(value, place.Type.ComponentKind(index), element));
    }

    private Value CbufferDword(Value index)
    {
        var handle = _context.Resources.UserData();
        var row = Builder.Binary(BinaryOp.LShr, index, Module.Int32(RowShift));
        var result = _context.Ops.CbufferRow(handle, row);

        if (index is Constant constant)
        {
            return Builder.Extract(result, (int)(constant.Unsigned % DwordsPerRow));
        }

        var column = Builder.Binary(BinaryOp.And, index, Module.Int32(DwordsPerRow - 1));
        var value = Builder.Extract(result, 0);

        for (var component = 1; component < DwordsPerRow; component++)
        {
            var matches = Builder.Compare(Predicate.Equal, column, Module.Int32((uint)component));
            value = Builder.Select(matches, Builder.Extract(result, component), value);
        }

        return value;
    }

    /// <summary>A loaded dword (or qword) as a component of <paramref name="kind"/>.</summary>
    public Value FromMemory(Value stored, ScalarKind kind)
    {
        return kind switch
        {
            ScalarKind.Float => Builder.Cast(CastOp.Bitcast, stored, Module.Types.Float),
            ScalarKind.Double => Builder.Cast(CastOp.Bitcast, stored, Module.Types.Double),
            ScalarKind.Bool => Builder.Compare(Predicate.NotEqual, stored, Module.Int(stored.Type, 0)),
            _ => stored,
        };
    }

    private Value ToMemory(Value value, ScalarKind kind, IrType element)
    {
        if (value is Constant constant && constant.Kind == ConstantKind.Undefined)
        {
            return Module.Undefined(element);
        }

        return kind switch
        {
            ScalarKind.Float or ScalarKind.Double => Bits(value, element),
            ScalarKind.Bool => Extend(value, element),
            _ => value,
        };
    }

    private Value Bits(Value value, IrType element)
    {
        if (value is Constant constant)
        {
            return Module.Int(element, constant.Bits);
        }

        return Builder.Cast(CastOp.Bitcast, value, element);
    }

    private Value Extend(Value value, IrType element)
    {
        if (value is Constant constant)
        {
            return Module.Int(element, constant.Bits & 1);
        }

        return Builder.Cast(CastOp.ZExt, value, element);
    }

    private IrType SlotType(HlslType type, int index)
    {
        if (type.Shape == TypeShape.Resource)
        {
            return _context.Ops.Handle;
        }

        return _context.Convert.IrTypeOf(type.ComponentKind(index));
    }

    private IrType MemoryElement(HlslType type)
    {
        var wide = Enumerable.Range(0, type.Count).Select(index => Conversions.IsWide(type.ComponentKind(index))).Distinct().ToList();

        if (wide.Count > 1)
        {
            throw new DxilBackendException($"an array mixing 32- and 64-bit components ({type}) is not supported");
        }

        if (wide[0])
        {
            return Module.Types.Int64;
        }

        return Module.Types.Int32;
    }

    private Value[] Counting(int start, int count)
    {
        return [.. Enumerable.Range(start, count).Select(index => (Value)Module.Int32((uint)index))];
    }
}
