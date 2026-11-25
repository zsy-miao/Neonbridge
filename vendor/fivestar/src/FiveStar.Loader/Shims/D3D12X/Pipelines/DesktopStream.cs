// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Security.Cryptography;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// A desktop pipeline-state stream: each subobject is a u32 D3D12_PIPELINE_STATE_SUBOBJECT_TYPE and its payload at
/// the payload's alignment, every subobject pointer-aligned. The stream keeps the shader bytecode and input elements it
/// was given; <see cref="Describe"/> lays them out in native memory, which <see cref="Release"/> frees again, so a
/// stream waiting for a build thread holds no copy of its DXIL. Alongside, a SHA-256 runs over the stream's content
/// with every pointer replaced by what it points to (the root signature by nothing: the cache's salt covers it), so
/// <see cref="Key"/> names the pipeline the same way in every process.
/// </summary>
internal sealed unsafe class DesktopStream : IDisposable
{
    /// <summary>Bytes of the SHA-256 kept in <see cref="Key"/>.</summary>
    private const int KeyBytes = 16;

    private const int WordSize = sizeof(int);
    private const int StreamDescWords = 2;
    private const int ElementFieldsOffset = PipelineLayout.PointerSize;

    private readonly List<byte> _bytes = [];
    private readonly List<(int Offset, byte[] Bytecode)> _shaders = [];
    private readonly List<(int Offset, IReadOnlyList<VertexElement> Elements)> _layouts = [];
    private readonly List<nint> _allocations = [];
    private readonly IncrementalHash _identity = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
    private nint* _desc;
    private string? _key;
    private int _rootOffset = -1;

    /// <summary>The longest stream, shader or table a stream from another process may hold.</summary>
    private const int MaximumLength = 1 << 30;

    /// <summary>The bytes of shader bytecode in the stream.</summary>
    public long ShaderBytes { get; private set; }

    /// <summary>The pipeline's name in the pipeline cache: 32 hex digits of the content hash.</summary>
    public string Key => _key ??= Convert.ToHexStringLower(_identity.GetCurrentHash().AsSpan(0, KeyBytes));

    public void AddPointer(PipelineStateSubObjectType type, nint address)
    {
        Begin(type, PipelineLayout.PointerSize);

        if (type == PipelineStateSubObjectType.RootSignature)
        {
            _rootOffset = _bytes.Count;
        }

        _bytes.AddRange(BitConverter.GetBytes((long)address));
    }

    /// <summary>
    /// Writes the stream for another process (<see cref="PipelineHost"/>): its bytes, its shaders and input layouts
    /// by offset, and where the root signature goes; pointers are filled in again by <see cref="Read"/>.
    /// </summary>
    public void Write(BinaryWriter writer)
    {
        Align(PipelineLayout.PointerSize);
        writer.Write(_bytes.Count);
        writer.Write(_bytes.ToArray());
        writer.Write(_rootOffset);
        writer.Write(_shaders.Count);

        foreach (var (offset, bytecode) in _shaders)
        {
            writer.Write(offset);
            writer.Write(bytecode.Length);
            writer.Write(bytecode);
        }

        writer.Write(_layouts.Count);

        foreach (var (offset, elements) in _layouts)
        {
            writer.Write(offset);
            writer.Write(elements.Count);

            foreach (var element in elements)
            {
                writer.Write(element.Semantic);
                Fields(element).ToList().ForEach(writer.Write);
            }
        }
    }

    /// <summary>
    /// A stream <see cref="Write"/> wrote, over <paramref name="rootSignature"/> (an ID3D12RootSignature of this
    /// process); <see cref="InvalidDataException"/> when a length or offset does not fit.
    /// </summary>
    public static DesktopStream Read(BinaryReader reader, nint rootSignature)
    {
        var stream = new DesktopStream();
        stream._bytes.AddRange(Bytes(reader, Length(reader)));
        stream._rootOffset = reader.ReadInt32();
        int size = stream._bytes.Count;

        if (stream._rootOffset != -1)
        {
            CheckPointer(stream._rootOffset, size);
            var pointer = BitConverter.GetBytes((long)rootSignature);

            for (var index = 0; index < pointer.Length; index++)
            {
                stream._bytes[stream._rootOffset + index] = pointer[index];
            }
        }

        for (var count = Length(reader); count > 0; count--)
        {
            var offset = reader.ReadInt32();
            CheckSlot(offset, size);
            var bytecode = Bytes(reader, Length(reader));
            stream._shaders.Add((offset, bytecode));
            stream.ShaderBytes += bytecode.Length;
        }

        for (var count = Length(reader); count > 0; count--)
        {
            var offset = reader.ReadInt32();
            CheckSlot(offset, size);
            var elements = new List<VertexElement>();

            for (var element = Length(reader); element > 0; element--)
            {
                elements.Add(new VertexElement(reader.ReadString(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32(), reader.ReadInt32()));
            }

            stream._layouts.Add((offset, elements));
        }

        return stream;
    }

    /// <summary>A D3D12_SHADER_BYTECODE subobject over <paramref name="bytecode"/>, which must not change afterwards.</summary>
    public void AddShader(PipelineStateSubObjectType type, byte[] bytecode)
    {
        Begin(type, PipelineLayout.PointerSize);
        _shaders.Add((_bytes.Count, bytecode));
        _bytes.AddRange(BitConverter.GetBytes(0L));
        _bytes.AddRange(BitConverter.GetBytes((long)bytecode.Length));

        ShaderBytes += bytecode.Length;
        Identify(BitConverter.GetBytes(bytecode.Length));
        Identify(bytecode);
    }

    /// <summary>A subobject whose payload is a desc with the d3d12.h layout.</summary>
    public void AddBytes(PipelineStateSubObjectType type, ReadOnlySpan<byte> payload)
    {
        Begin(type, WordSize);
        _bytes.AddRange(payload);
        Identify(payload);
    }

    public void AddWords(PipelineStateSubObjectType type, IReadOnlyList<int> words)
    {
        Begin(type, WordSize);

        foreach (var word in words)
        {
            var bytes = BitConverter.GetBytes(word);
            _bytes.AddRange(bytes);
            Identify(bytes);
        }
    }

    /// <summary>A D3D12_INPUT_LAYOUT_DESC over D3D12_INPUT_ELEMENT_DESCs of <paramref name="elements"/>.</summary>
    public void AddInputLayout(IReadOnlyList<VertexElement> elements)
    {
        Identify(BitConverter.GetBytes(elements.Count));

        foreach (var element in elements)
        {
            Identify(System.Text.Encoding.ASCII.GetBytes(element.Semantic + "\0"));

            foreach (var field in Fields(element))
            {
                Identify(BitConverter.GetBytes(field));
            }
        }

        Begin(PipelineStateSubObjectType.InputLayout, PipelineLayout.PointerSize);
        _layouts.Add((_bytes.Count, elements));
        _bytes.AddRange(BitConverter.GetBytes(0L));
        _bytes.AddRange(BitConverter.GetBytes((long)elements.Count));
    }

    /// <summary>
    /// The D3D12_PIPELINE_STATE_STREAM_DESC {SizeInBytes, pPipelineStateSubobjectStream} over a native copy of the
    /// stream, valid until <see cref="Release"/>; no subobject may be added after the first call.
    /// </summary>
    public void* Describe()
    {
        if (_desc != null)
        {
            return _desc;
        }

        Align(PipelineLayout.PointerSize);
        var stream = (byte*)Native(_bytes.ToArray());

        foreach (var (offset, bytecode) in _shaders)
        {
            CheckSlot(offset, _bytes.Count);
            *(nint*)(stream + offset) = Native(bytecode);
        }

        foreach (var (offset, elements) in _layouts)
        {
            CheckSlot(offset, _bytes.Count);
            *(nint*)(stream + offset) = InputElements(elements);
        }

        var desc = (nint*)NativeMemory.Alloc((nuint)(StreamDescWords * sizeof(nint)));
        _allocations.Add((nint)desc);

        desc[0] = _bytes.Count;
        desc[1] = (nint)stream;
        _desc = desc;

        return desc;
    }

    /// <summary>ID3D12Device2::CreatePipelineState over the stream.</summary>
    public int Create(nint device2, Guid* iid, nint* result)
    {
        return DesktopDevice.CreatePipelineState(device2, Describe(), iid, result);
    }

    /// <summary>Frees the native copy <see cref="Describe"/> made; the next call makes a new one.</summary>
    public void Release()
    {
        foreach (var allocation in _allocations)
        {
            NativeMemory.Free((void*)allocation);
        }

        _allocations.Clear();
        _desc = null;
    }

    public void Dispose()
    {
        Release();
        _identity.Dispose();
    }

    /// <summary>
    /// A count or length read from a stream: not negative, not past <see cref="MaximumLength"/>, and on a stream that
    /// can tell (a file, memory) not past what is left of it. The pipeline host reads its request from a pipe, which
    /// has no length: asking it ended every host of boot w28 before it built anything.
    /// </summary>
    private static int Length(BinaryReader reader)
    {
        int length = reader.ReadInt32();
        var source = reader.BaseStream;

        if (length < 0 || length > MaximumLength || (source.CanSeek && length > source.Length - source.Position))
        {
            throw new InvalidDataException($"a length of {length} in a pipeline stream");
        }

        return length;
    }

    /// <summary><paramref name="count"/> bytes; <see cref="InvalidDataException"/> when the stream ends first.</summary>
    private static byte[] Bytes(BinaryReader reader, int count)
    {
        var bytes = reader.ReadBytes(count);

        if (bytes.Length != count)
        {
            throw new InvalidDataException($"a pipeline stream ends {count - bytes.Length} bytes early");
        }

        return bytes;
    }

    /// <summary>A pointer-sized field at <paramref name="offset"/> lies inside a stream of <paramref name="size"/> bytes.</summary>
    private static void CheckPointer(int offset, int size)
    {
        if (offset < 0 || offset > size - PipelineLayout.PointerSize)
        {
            throw new InvalidDataException($"a pointer at {offset} in a pipeline stream of {size} bytes");
        }
    }

    /// <summary>A pointer and its size (D3D12_SHADER_BYTECODE, D3D12_INPUT_LAYOUT_DESC) at <paramref name="offset"/> lie inside the stream.</summary>
    private static void CheckSlot(int offset, int size)
    {
        CheckPointer(offset, size - PipelineLayout.PointerSize);
    }

    /// <summary>A native D3D12_INPUT_ELEMENT_DESC table: semantic name pointer, then the six u32 fields.</summary>
    private nint InputElements(IReadOnlyList<VertexElement> elements)
    {
        var table = new byte[elements.Count * PipelineLayout.InputElementSize];

        for (var index = 0; index < elements.Count; index++)
        {
            var row = table.AsSpan(index * PipelineLayout.InputElementSize);
            var fields = Fields(elements[index]);
            BitConverter.TryWriteBytes(row, (long)NativeString(elements[index].Semantic));

            for (var field = 0; field < fields.Length; field++)
            {
                BitConverter.TryWriteBytes(row[(ElementFieldsOffset + (field * WordSize))..], fields[field]);
            }
        }

        return Native(table);
    }

    private static int[] Fields(VertexElement element)
    {
        return [element.SemanticIndex, element.Format, element.InputSlot, element.Offset, element.Classification, element.StepRate];
    }

    private nint Native(byte[] bytes)
    {
        var copy = (byte*)NativeMemory.Alloc((nuint)Math.Max(bytes.Length, 1));
        bytes.CopyTo(new Span<byte>(copy, bytes.Length));
        _allocations.Add((nint)copy);

        return (nint)copy;
    }

    private nint NativeString(string text)
    {
        var bytes = new byte[text.Length + 1];
        System.Text.Encoding.ASCII.GetBytes(text, bytes);

        return Native(bytes);
    }

    private void Begin(PipelineStateSubObjectType type, int payloadAlignment)
    {
        Align(PipelineLayout.PointerSize);
        var tag = BitConverter.GetBytes((int)type);
        _bytes.AddRange(tag);
        Identify(tag);
        Align(payloadAlignment);
    }

    private void Identify(ReadOnlySpan<byte> content)
    {
        _key = null;
        _identity.AppendData(content);
    }

    private void Align(int alignment)
    {
        while (_bytes.Count % alignment != 0)
        {
            _bytes.Add(0);
        }
    }
}
