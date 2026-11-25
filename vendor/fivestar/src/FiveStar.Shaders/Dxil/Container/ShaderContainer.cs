// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// Finishes a module: the DXIL metadata (versions, shader model, the user-data constant buffer, the signatures and view-ID
/// state of graphics stages, the entry point with its properties), the bitcode, and the container (SFI0, ISG1, OSG1,
/// PSG1, PSV0, DXIL) with its digest. Shader model 6.7 and validator 1.7, as dxc compiles the translator's programs
/// (pixel shaders count helper lanes in wave operations, SampleCmpLevel).
/// </summary>
public static class ShaderContainer
{
    public const string UserDataName = "UserData";

    private const uint Major = 1;
    private const uint Minor = 7;
    private const uint ModelMajor = 6;
    private const uint FlagsTag = 0;
    private const uint GeometryTag = 1;
    private const uint ThreadsTag = 4;
    private const uint MeshTag = 9;
    private const uint WaveSizeTag = 11;
    private const uint UsageTag = 3;
    private const uint CbufferKind = 13;
    private const uint PsvCbuffer = 2;
    private const int KindShift = 16;
    private const int MajorShift = 4;
    private const int ProgramHeaderBytes = 24;
    private const int BitcodeHeaderBytes = 16;
    private const int UserDataWords = 128;
    private const uint DxilMajorShift = 8;

    private static readonly (ShaderFlagBits Flag, ulong Feature)[] Features =
    [
        (ShaderFlagBits.EnableDoublePrecision, 0x1),
        (ShaderFlagBits.EnableDoubleExtensions, 0x20),
        (ShaderFlagBits.StencilExport, 0x200),
        (ShaderFlagBits.TypedUavLoadAdditionalFormats, 0x800),
        (ShaderFlagBits.WaveOps, 0x4000),
        (ShaderFlagBits.Int64Ops, 0x8000),
        (ShaderFlagBits.Barycentrics, 0x20000),
        (ShaderFlagBits.AtomicInt64OnTypedResource, 0x40_0000),
        (ShaderFlagBits.AtomicInt64OnGroupShared, 0x80_0000),
        (ShaderFlagBits.ResourceDescriptorHeapIndexing, 0x200_0000),
        (ShaderFlagBits.SamplerDescriptorHeapIndexing, 0x400_0000),
        (ShaderFlagBits.AtomicInt64OnHeapResource, 0x1000_0000),
        (ShaderFlagBits.AdvancedTextureOps, 0x2000_0000),
    ];

    private static readonly Dictionary<DxilShaderKind, string> Models = new()
    {
        [DxilShaderKind.Pixel] = "ps",
        [DxilShaderKind.Vertex] = "vs",
        [DxilShaderKind.Geometry] = "gs",
        [DxilShaderKind.Compute] = "cs",
        [DxilShaderKind.Mesh] = "ms",
    };

    public static byte[] Write(DxilModule module, IrFunction main, ShaderDescription description)
    {
        var bindings = new List<PsvBinding>();
        var resources = Resources(module, description, bindings);
        Metadata? signatures = null;

        if (description.IsGraphics)
        {
            signatures = module.Node(Signature(module, description.Inputs), Signature(module, description.Outputs), Signature(module, description.Primitives));
        }

        var entry = module.Node(module.ValueMetadata(main), module.Text("main"), signatures, resources, Properties(module, description));
        module.Named("llvm.ident", module.Node(module.Text("Five Star Recomp")));
        var version = module.Node(module.Int32Metadata(Major), module.Int32Metadata(Minor));
        module.Named("dx.version", version);

        module.Named("dx.valver", version);
        module.Named("dx.shaderModel", module.Node(module.Text(Models[description.Kind]), module.Int32Metadata(ModelMajor), module.Int32Metadata(Minor)));

        if (resources is not null)
        {
            module.Named("dx.resources", resources);
        }

        if (description.IsGraphics)
        {
            var state = ViewIdState.Serialize(description);
            var array = module.Types.Array(state.Length, module.Types.Int32);
            var words = module.Aggregate(array, [.. state.Select(module.Int32)]);
            module.Named("dx.viewIdState", module.Node(module.ValueMetadata(words)));
        }

        module.Named("dx.entryPoints", entry);
        var program = Program(ModuleWriter.Write(module), description.Kind);
        var parts = new List<(string FourCc, byte[] Data)>
        {
            ("SFI0", Sfi0(description.Flags)),
            ("ISG1", SignatureWriter.Write(description.Inputs, input: true)),
            ("OSG1", SignatureWriter.Write(description.Outputs, input: false)),
        };

        if (description.Kind == DxilShaderKind.Mesh)
        {
            parts.Add(("PSG1", SignatureWriter.Write(description.Primitives, input: false)));
        }

        parts.Add(("PSV0", PsvWriter.Write(description, bindings)));
        parts.Add(("DXIL", program));

        return ContainerWriter.Build(parts);
    }

    private static Metadata? Resources(DxilModule module, ShaderDescription description, List<PsvBinding> bindings)
    {
        if (description.UserDataBytes == 0)
        {
            return null;
        }

        var types = module.Types;
        var layout = types.Struct(UserDataName, [types.Array(UserDataWords, types.Int32)]);
        var pointer = module.Undefined(types.Pointer(layout, 0));
        var cbuffer = module.Node(
            module.Int32Metadata(0), module.ValueMetadata(pointer), module.Text(string.Empty), module.Int32Metadata(0),
            module.Int32Metadata(0), module.Int32Metadata(1), module.Int32Metadata((uint)description.UserDataBytes), null);
        bindings.Add(new PsvBinding(PsvCbuffer, 0, 0, CbufferKind));

        return module.Node(null, null, module.Node(cbuffer), null);
    }

    /// <summary>
    /// The elements of one signature: id, name, component type, kind, indices, interpolation, rows, columns, start row
    /// and column (-1: none), and the components used.
    /// </summary>
    private static Metadata? Signature(DxilModule module, IReadOnlyList<DxilSignatureElement> elements)
    {
        var listed = elements.Where(element => element.InSignature).ToList();

        if (listed.Count == 0)
        {
            return null;
        }

        var nodes = new Metadata?[listed.Count];

        for (var index = 0; index < listed.Count; index++)
        {
            var element = listed[index];
            var indices = module.Node([.. Enumerable.Range(element.Index, element.Rows).Select(semantic => module.Int32Metadata((uint)semantic))]);
            Metadata? usage = null;

            if (element.Usage != 0)
            {
                usage = module.Node(module.Int32Metadata(UsageTag), module.Int32Metadata(element.Usage));
            }

            nodes[index] = module.Node(
                module.Int32Metadata((uint)element.Id), module.Text(element.Name), Int8(module, element.ComponentType), Int8(module, (byte)element.Kind),
                indices, Int8(module, element.Interpolation), module.Int32Metadata((uint)element.Rows), Int8(module, (byte)element.Columns),
                module.Int32Metadata((uint)element.StartRow), Int8(module, (byte)element.StartColumn), usage);
        }

        return module.Node(nodes);
    }

    private static Metadata Int8(DxilModule module, byte value)
    {
        return module.ValueMetadata(module.Int8(value));
    }

    private static Metadata? Properties(DxilModule module, ShaderDescription description)
    {
        var properties = new List<Metadata?>();

        if (description.Flags != ShaderFlagBits.None)
        {
            properties.Add(module.Int32Metadata(FlagsTag));
            properties.Add(module.ValueMetadata(module.Int(module.Types.Int64, (long)description.Flags)));
        }

        var (x, y, z) = description.Threads;
        var threads = module.Node(module.Int32Metadata((uint)x), module.Int32Metadata((uint)y), module.Int32Metadata((uint)z));

        if (description.Kind == DxilShaderKind.Compute)
        {
            properties.Add(module.Int32Metadata(ThreadsTag));
            properties.Add(threads);
        }

        StageProperties(module, description, threads, properties);

        if (description.WaveSize is not null)
        {
            properties.Add(module.Int32Metadata(WaveSizeTag));
            properties.Add(module.Node(module.Int32Metadata((uint)description.WaveSize.Value)));
        }

        if (properties.Count == 0)
        {
            return null;
        }

        return module.Node([.. properties]);
    }

    /// <summary>
    /// The mesh state (threads, maximum vertices and primitives, topology, payload bytes) or the geometry state (input
    /// primitive, maximum vertices, stream mask, topology, instances).
    /// </summary>
    private static void StageProperties(DxilModule module, ShaderDescription description, Metadata threads, List<Metadata?> properties)
    {
        var mesh = description.Mesh;
        var geometry = description.Geometry;

        if (mesh is not null)
        {
            properties.Add(module.Int32Metadata(MeshTag));
            properties.Add(module.Node(
                threads, module.Int32Metadata((uint)mesh.MaxVertices), module.Int32Metadata((uint)mesh.MaxPrimitives),
                module.Int32Metadata((uint)mesh.Topology), module.Int32Metadata(0)));
        }

        if (geometry is not null)
        {
            properties.Add(module.Int32Metadata(GeometryTag));
            properties.Add(module.Node(
                module.Int32Metadata((uint)geometry.InputPrimitive), module.Int32Metadata((uint)geometry.MaxVertexCount),
                module.Int32Metadata((uint)geometry.StreamMask), module.Int32Metadata((uint)geometry.OutputTopology), module.Int32Metadata(1)));
        }
    }

    /// <summary>The DXIL part: program header (kind, version, size in dwords), bitcode header, bitcode.</summary>
    private static byte[] Program(byte[] bitcode, DxilShaderKind kind)
    {
        var bytes = new byte[ProgramHeaderBytes + bitcode.Length];
        var version = ((uint)kind << KindShift) | (ModelMajor << MajorShift) | Minor;
        BinaryPrimitives.WriteUInt32LittleEndian(bytes, version);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(sizeof(uint)), (uint)(bytes.Length / sizeof(uint)));
        "DXIL"u8.CopyTo(bytes.AsSpan(sizeof(ulong)));
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(sizeof(ulong) + sizeof(uint)), (Major << (int)DxilMajorShift) | Minor);

        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(BitcodeHeaderBytes), BitcodeHeaderBytes);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(BitcodeHeaderBytes + sizeof(uint)), (uint)bitcode.Length);
        bitcode.CopyTo(bytes, ProgramHeaderBytes);

        return bytes;
    }

    private static byte[] Sfi0(ShaderFlagBits flags)
    {
        ulong features = 0;

        foreach (var (flag, feature) in Features)
        {
            if (flags.HasFlag(flag))
            {
                features |= feature;
            }
        }

        return BitConverter.GetBytes(features);
    }
}
