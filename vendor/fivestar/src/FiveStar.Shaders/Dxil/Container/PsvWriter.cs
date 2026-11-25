// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Text;

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// PSV0 (pipeline state validation) with runtime info version 2, as validator 1.6 and 1.7 write it: runtime info, the
/// bound resources, the string and semantic index tables, the signature elements, and the input-to-output dependency
/// table of the view-ID state.
/// </summary>
public static class PsvWriter
{
    private const int RuntimeInfoBytes = 48;
    private const int BindingBytes = 24;
    private const int ElementBytes = 16;
    private const int Components = 4;
    private const int BitsPerWord = 32;

    // PSVRuntimeInfo0..2 (DxilPipelineStateValidation.h): the stage union @0 (VS: output position present; PS: depth
    // output, sample frequency; MS: shared bytes, then max vertices and primitives @12; GS: input primitive, output
    // topology, stream mask, output position present @12), wave sizes @16 and @20, stage @24, the union @26 (GS: max
    // vertex count; MS: primitive vectors, topology), element counts @28, input vectors @31, output vectors @32, threads @36.
    private const int SampleFrequencyOffset = 1;
    private const int MeshVerticesOffset = 12;
    private const int GeometryPositionOffset = 12;
    private const int MinimumWaveOffset = 16;
    private const int MaximumWaveOffset = 20;
    private const int StageOffset = 24;
    private const int UnionOffset = 26;
    private const int InputElementsOffset = 28;
    private const int OutputElementsOffset = 29;
    private const int PrimitiveElementsOffset = 30;
    private const int InputVectorsOffset = 31;
    private const int OutputVectorsOffset = 32;
    private const int ThreadsOffset = 36;

    // PSVSignatureElement0: name @0, semantic indices @4, rows @8, start row @9, columns (bits 0-3), start column
    // (bits 4-5) and allocated (bit 6) @10, kind @11, component type @12, interpolation @13.
    private const int RowsField = 8;
    private const int StartRowField = 9;
    private const int ColumnsField = 10;
    private const int KindField = 11;
    private const int ComponentTypeField = 12;
    private const int InterpolationField = 13;
    private const byte Allocated = 0x40;
    private const int ColumnShift = 4;

    public static byte[] Write(ShaderDescription description, IReadOnlyList<PsvBinding> bindings)
    {
        var output = new List<byte>();
        AppendWords(output, RuntimeInfoBytes);
        output.AddRange(RuntimeInfo(description));
        AppendWords(output, (uint)bindings.Count);

        if (bindings.Count > 0)
        {
            AppendWords(output, BindingBytes);

            foreach (var binding in bindings)
            {
                AppendWords(output, binding.Type, binding.Space, binding.Register, binding.Register, binding.Kind, 0);
            }
        }

        Elements(output, description);
        Dependencies(output, description);

        return [.. output];
    }

    private static byte[] RuntimeInfo(ShaderDescription description)
    {
        var info = new byte[RuntimeInfoBytes];
        var minimum = 0u;
        var maximum = uint.MaxValue;

        if (description.WaveSize is not null)
        {
            minimum = (uint)description.WaveSize.Value;
            maximum = minimum;
        }

        BinaryPrimitives.WriteUInt32LittleEndian(info.AsSpan(MinimumWaveOffset), minimum);
        BinaryPrimitives.WriteUInt32LittleEndian(info.AsSpan(MaximumWaveOffset), maximum);
        info[StageOffset] = (byte)description.Kind;
        StageInfo(info, description);

        info[InputElementsOffset] = (byte)description.Inputs.Count(element => element.InSignature);
        info[OutputElementsOffset] = (byte)description.Outputs.Count(element => element.InSignature);
        info[PrimitiveElementsOffset] = (byte)description.Primitives.Count(element => element.InSignature);
        info[InputVectorsOffset] = (byte)ViewIdState.Vectors(description.Inputs);
        info[OutputVectorsOffset] = (byte)ViewIdState.Vectors(description.Outputs);

        if (description.Kind is DxilShaderKind.Compute or DxilShaderKind.Mesh)
        {
            var (x, y, z) = description.Threads;
            BinaryPrimitives.WriteUInt32LittleEndian(info.AsSpan(ThreadsOffset), (uint)x);
            BinaryPrimitives.WriteUInt32LittleEndian(info.AsSpan(ThreadsOffset + sizeof(uint)), (uint)y);
            BinaryPrimitives.WriteUInt32LittleEndian(info.AsSpan(ThreadsOffset + sizeof(ulong)), (uint)z);
        }

        return info;
    }

    /// <summary>The stage's own fields at the start of the runtime info, and the union after the stage byte.</summary>
    private static void StageInfo(byte[] info, ShaderDescription description)
    {
        var outputs = description.Outputs;
        var position = Flag(outputs.Any(element => element.Kind == SemanticKind.Position));

        switch (description.Kind)
        {
            case DxilShaderKind.Vertex:
                info[0] = position;
                break;
            case DxilShaderKind.Pixel:
                info[0] = Flag(outputs.Any(element => element.Kind is SemanticKind.Depth or SemanticKind.DepthLessEqual or SemanticKind.DepthGreaterEqual));
                info[SampleFrequencyOffset] = Flag(description.Inputs.Any(element => element.Kind == SemanticKind.SampleIndex
                    || element.Interpolation is Semantics.LinearSample or Semantics.LinearNoPerspectiveSample));
                break;
            case DxilShaderKind.Mesh:
                MeshInfo(info, description);
                break;
            case DxilShaderKind.Geometry:
                GeometryInfo(info, description.Geometry!, position);
                break;
        }
    }

    private static void MeshInfo(byte[] info, ShaderDescription description)
    {
        var mesh = description.Mesh!;
        BinaryPrimitives.WriteUInt32LittleEndian(info, (uint)mesh.SharedBytes);
        BinaryPrimitives.WriteUInt16LittleEndian(info.AsSpan(MeshVerticesOffset), (ushort)mesh.MaxVertices);
        BinaryPrimitives.WriteUInt16LittleEndian(info.AsSpan(MeshVerticesOffset + sizeof(ushort)), (ushort)mesh.MaxPrimitives);
        info[UnionOffset] = (byte)ViewIdState.Vectors(description.Primitives);
        info[UnionOffset + 1] = (byte)mesh.Topology;
    }

    private static void GeometryInfo(byte[] info, GeometryState geometry, byte position)
    {
        BinaryPrimitives.WriteUInt32LittleEndian(info, (uint)geometry.InputPrimitive);
        BinaryPrimitives.WriteUInt32LittleEndian(info.AsSpan(sizeof(uint)), (uint)geometry.OutputTopology);
        BinaryPrimitives.WriteUInt32LittleEndian(info.AsSpan(sizeof(ulong)), (uint)geometry.StreamMask);
        info[GeometryPositionOffset] = position;
        BinaryPrimitives.WriteUInt16LittleEndian(info.AsSpan(UnionOffset), (ushort)geometry.MaxVertexCount);
    }

    private static byte Flag(bool value)
    {
        byte flag = 0;

        if (value)
        {
            flag = 1;
        }

        return flag;
    }

    /// <summary>The string table, the semantic index table and the elements (inputs, outputs, primitives).</summary>
    private static void Elements(List<byte> output, ShaderDescription description)
    {
        var elements = description.Inputs.Concat(description.Outputs).Concat(description.Primitives).Where(element => element.InSignature).ToList();
        var strings = new List<byte> { 0 };
        var indices = new List<uint>();
        var rows = elements.Select(element => Element(element, strings, indices)).ToList();

        while (strings.Count % sizeof(uint) != 0)
        {
            strings.Add(0);
        }

        AppendWords(output, (uint)strings.Count);
        output.AddRange(strings);
        AppendWords(output, (uint)indices.Count);
        AppendWords(output, [.. indices]);

        if (rows.Count > 0)
        {
            AppendWords(output, ElementBytes);
            rows.ForEach(output.AddRange);
        }
    }

    /// <summary>One element: name and index offsets, rows, start row, columns, kind, type, interpolation.</summary>
    private static byte[] Element(DxilSignatureElement element, List<byte> strings, List<uint> indices)
    {
        var bytes = new byte[ElementBytes];
        uint name = 0;

        if (element.Kind == SemanticKind.Arbitrary)
        {
            name = (uint)strings.Count;
            strings.AddRange(Encoding.ASCII.GetBytes(element.Name));
            strings.Add(0);
        }

        var semanticIndices = Enumerable.Range(element.Index, element.Rows).Select(index => (uint)index).ToArray();
        BinaryPrimitives.WriteUInt32LittleEndian(bytes, name);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(sizeof(uint)), (uint)IndexOffset(indices, semanticIndices));
        bytes[RowsField] = (byte)element.Rows;
        bytes[StartRowField] = (byte)Math.Max(element.StartRow, 0);
        bytes[ColumnsField] = (byte)element.Columns;

        if (element.IsAllocated)
        {
            bytes[ColumnsField] |= (byte)(Allocated | (element.StartColumn << ColumnShift));
        }

        bytes[KindField] = (byte)element.Kind;
        bytes[ComponentTypeField] = element.RegisterComponentType;
        bytes[InterpolationField] = element.Interpolation;

        return bytes;
    }

    /// <summary>Where <paramref name="sequence"/> starts in the index table, appending it if it is not there.</summary>
    private static int IndexOffset(List<uint> indices, uint[] sequence)
    {
        for (var start = 0; start + sequence.Length <= indices.Count; start++)
        {
            if (indices.Skip(start).Take(sequence.Length).SequenceEqual(sequence))
            {
                return start;
            }
        }

        indices.AddRange(sequence);

        return indices.Count - sequence.Length;
    }

    /// <summary>The input-to-output table (stream 0) the view-ID state declares: every output on every input.</summary>
    private static void Dependencies(List<byte> output, ShaderDescription description)
    {
        var inputVectors = ViewIdState.Vectors(description.Inputs);
        var outputVectors = ViewIdState.Vectors(description.Outputs);

        if (inputVectors == 0 || outputVectors == 0)
        {
            return;
        }

        var inputs = ViewIdState.Scalars(description.Inputs);
        var outputs = ViewIdState.Scalars(description.Outputs);
        var stride = ((outputVectors * Components) + BitsPerWord - 1) / BitsPerWord;

        for (var input = 0; input < inputVectors * Components; input++)
        {
            for (var word = 0; word < stride; word++)
            {
                var bits = 0u;

                if (input < inputs)
                {
                    bits = ViewIdState.Dependencies(word, outputs);
                }

                AppendWords(output, bits);
            }
        }
    }

    private static void AppendWords(List<byte> output, params uint[] values)
    {
        Span<byte> word = stackalloc byte[sizeof(uint)];

        foreach (var value in values)
        {
            BinaryPrimitives.WriteUInt32LittleEndian(word, value);
            output.AddRange(word.ToArray());
        }
    }
}
