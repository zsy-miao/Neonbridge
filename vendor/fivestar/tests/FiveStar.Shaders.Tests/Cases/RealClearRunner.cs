// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Tests.Reference;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// The executable's VS_Clear from the user's dump, end to end. It builds a V# from the root SRV address in s8:s9 (dword 3
/// 0x11014fac) and reads a record of it: dword 0 a header for the positions (low nibble 3, offset and stride in its upper
/// bits), dword 3 one for the colours (nibble 4). The record is laid out as in the boot's first clear (positions from
/// byte 16, colours from byte 28, 28 bytes per vertex) with a full-screen triangle and a colour that is not black, at a
/// console address the address map sends to a buffer. Every pixel must get the colour.
/// </summary>
public sealed class RealClearRunner
{
    private const int Size = 8;
    private const ulong RootAddress = 0x27E_0001_0040;
    private const int RecordHeap = 10;
    private const int NullView = 8;
    private const int PageBytes = 0x10000;
    private const int PositionHeader = 0x8E3;
    private const int ColourHeader = 0xEE4;
    private const int VertexDwords = 7;
    private const int FirstPosition = 4;
    private const int FirstColour = 7;

    private static readonly double[][] Positions = [[-1.0, 1.0, 0.0], [-1.0, -3.0, 0.0], [3.0, 1.0, 0.0]];
    private static readonly uint[] Colour = [FloatBits.Of(0.25), FloatBits.Of(0.5), FloatBits.Of(0.75), FloatBits.Of(1.0)];

    private readonly WarpFixture _fixture;

    public RealClearRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    /// <summary>Runs the program whose file name in <paramref name="folder"/> contains <paramref name="name"/>.</summary>
    public IReadOnlyList<string> Run(string folder, string name)
    {
        var path = Directory.GetFiles(folder, $"*{name}*.fssm").FirstOrDefault();

        if (path is null)
        {
            return [$"no *{name}*.fssm in {folder}"];
        }

        var loaded = new ProgramLoader(new RdnaDecoder()).Load(path, null);
        var program = loaded.Program ?? throw new InvalidOperationException($"{loaded.Status}: {loaded.Reason}");
        var hlsl = ShaderWorkbench.Translate(program);
        var (pixelSource, _) = PassThroughPixelShader.Build(hlsl, null, flat: true);
        var job = new GraphicsJob(
            _fixture.Workbench.Compile("vs_real_clear", hlsl, "vs"),
            _fixture.Workbench.Compile("vs_real_clear_ps", pixelSource, "ps"),
            Framebuffer.Exact(1, Size, Size),
            Bindings());

        var result = _fixture.Runner.Graphics(job);

        return ImageCheck.Compare(result, Size, Size, (_, _, _) => [.. Colour.Select(Expected.Exact)]);
    }

    private static Bindings Bindings()
    {
        var page = RootAddress & ~(ulong)(PageBytes - 1);
        var map = new AddressMap(1, NullView);
        var bytes = new byte[map.Bytes];
        map.Clear(bytes);
        map.Map(bytes, page, PageBytes, RecordHeap, page);

        var user = new uint[10];
        user[8] = unchecked((uint)RootAddress);
        user[9] = (uint)(RootAddress >> 32);

        return new Bindings
        {
            UserSgprs = user,
            Buffers =
            [
                new BufferResource((int)AddressMap.HeapSlot, MemoryMarshal.Cast<byte, uint>(bytes).ToArray(), Writable: false),
                new BufferResource(RecordHeap, Record((int)(RootAddress - page)), Writable: false),
            ],
        };
    }

    /// <summary>The buffer behind the page: the record at byte <paramref name="start"/>.</summary>
    private static uint[] Record(int start)
    {
        var words = new uint[(start / sizeof(uint)) + (VertexDwords * (Positions.Length + 1))];
        var record = words.AsSpan(start / sizeof(uint));
        record[0] = PositionHeader;
        record[3] = ColourHeader;

        for (var vertex = 0; vertex < Positions.Length; vertex++)
        {
            for (var axis = 0; axis < Positions[vertex].Length; axis++)
            {
                record[FirstPosition + (vertex * VertexDwords) + axis] = FloatBits.Of(Positions[vertex][axis]);
            }

            Colour.CopyTo(record[(FirstColour + (vertex * VertexDwords))..]);
        }

        return words;
    }
}
