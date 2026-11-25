// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Formats.Audio.Xma;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The SHAPE side of acphal.dll: the XMA decode contexts and the IAcpHal object the game sends context messages
/// through. The game uses SHAPE only to decode XMA (no SRC, EQ, filter or DMA contexts). The hardware decodes on its
/// own while a context is enabled; here each context decodes when a message changes it (enable, or output taken),
/// on the caller's thread, until its output ring is full or its input used up (<see cref="XmaContextDecoder"/>).
/// </summary>
internal sealed unsafe class AcpHalDevice : IDisposable
{
    private readonly object _gate = new();
    private readonly HashSet<uint> _loggedMessages = [];
    private readonly ApuHeap _heap;
    private readonly AudioCounters _counters;
    private readonly LoaderLog _log;
    private XmaContextDecoder?[] _decoders = [];
    private nint _object;
    private nint _vtable;
    private nint _contexts;

    public AcpHalDevice(ApuHeap heap, AudioCounters counters, LoaderLog log)
    {
        _heap = heap;
        _counters = counters;
        _log = log;
    }

    /// <summary>HRESULT AcpHalAllocateShapeContexts(SHAPE_CONTEXTS*): fills the XMA array pointer from the APU heap.</summary>
    public int AllocateContexts(byte* block)
    {
        if (block == null)
        {
            return HResult.Pointer;
        }

        var counts = new ReadOnlySpan<uint>(block, AcpHalLayout.ContextKinds);

        for (int kind = 0; kind < AcpHalLayout.ContextKinds; kind++)
        {
            if (kind != AcpHalLayout.XmaKind && counts[kind] != 0)
            {
                _log.Warn($"[acphal] AcpHalAllocateShapeContexts: {counts[kind]} contexts of kind {kind}; only XMA is known");

                return HResult.NotImplemented;
            }
        }

        uint xma = counts[AcpHalLayout.XmaKind];
        nint array = 0;

        if (xma != 0)
        {
            int hr = _heap.Allocate(&array, null, xma * AcpHalLayout.XmaContextBytes, AcpHalLayout.ContextAlignment);

            if (HResult.Failed(hr))
            {
                return hr;
            }
        }

        ((nint*)(block + AcpHalLayout.PointersOffset))[AcpHalLayout.XmaKind] = array;

        lock (_gate)
        {
            _contexts = array;
            _decoders = new XmaContextDecoder?[xma];
        }

        _log.Info($"  [acphal] AcpHalAllocateShapeContexts: {xma} XMA contexts at 0x{array:x} (decoded on the CPU)");

        return HResult.Ok;
    }

    /// <summary>AcpHalReleaseShapeContexts(): the game passes nothing, so the last array is freed.</summary>
    public void ReleaseContexts()
    {
        lock (_gate)
        {
            if (_contexts != 0)
            {
                _ = _heap.Free(_contexts);
                _contexts = 0;
                _decoders = [];
            }
        }
    }

    /// <summary>HRESULT AcpHalCreate(IAcpHal**): one object for the process.</summary>
    public int Create(nint* result)
    {
        if (result == null)
        {
            return HResult.Pointer;
        }

        lock (_gate)
        {
            if (_object == 0)
            {
                _vtable = (nint)NativeMemory.AllocZeroed(AcpHalLayout.Slots, (nuint)sizeof(nint));
                AcpHalMethods.Fill((nint*)_vtable);
                _object = (nint)NativeMemory.AllocZeroed((nuint)sizeof(nint));
                *(nint*)_object = _vtable;
            }
        }

        *result = _object;

        return HResult.Ok;
    }

    /// <summary>IAcpHal::SendMessage: the XMA context messages act on their context; the first message of each type is logged.</summary>
    public int SendMessage(uint type, uint cookie, uint* payload)
    {
        LogFirst(type, cookie, payload);

        lock (_gate)
        {
            switch ((AcpHalMessage)type)
            {
                case AcpHalMessage.EnableContext:
                    return EnableContext(payload);
                case AcpHalMessage.DisableContext:
                    return DisableContext(payload);
                case AcpHalMessage.ConsumeOutput:
                    return ConsumeOutput(payload);
                default:
                    return HResult.Ok;
            }
        }
    }

    public void Dispose()
    {
        NativeMemory.Free((void*)_object);
        NativeMemory.Free((void*)_vtable);
        _object = 0;
        _vtable = 0;
    }

    private int EnableContext(uint* payload)
    {
        uint* context = Context(payload);

        if (context == null)
        {
            return HResult.InvalidArgument;
        }

        int index = (int)*payload;
        var decoder = _decoders[index] ??= new XmaContextDecoder();
        var work = (uint*)_heap.ToVirtualAddress(XmaContextLayout.WorkBuffer.Read(context), XmaContextLayout.WorkBufferBytes);
        decoder.Enable(context, Chained(XmaWorkBufferTag.Read(work), index));
        new XmaWorkBufferTag((uint)index, decoder.Generation).Write(work);

        _counters.XmaEnabled();
        Service(decoder, context);

        return HResult.Ok;
    }

    /// <summary>
    /// The decoder a context continues: the one its work buffer's tag names, when that context is another one and has
    /// not been enabled again since it wrote the tag.
    /// </summary>
    private XmaContextDecoder? Chained(XmaWorkBufferTag? tag, int index)
    {
        if (tag is null)
        {
            return null;
        }

        var value = tag.Value;

        if (value.Context == (uint)index || value.Context >= (uint)_decoders.Length)
        {
            return null;
        }

        var previous = _decoders[value.Context];

        if (previous is null || previous.Generation != value.Generation)
        {
            return null;
        }

        return previous;
    }

    private int DisableContext(uint* payload)
    {
        if (Context(payload) == null)
        {
            return HResult.InvalidArgument;
        }

        _decoders[(int)*payload]?.Disable();
        _counters.XmaDisabled();

        return HResult.Ok;
    }

    /// <summary>Message 0x12: the game took blocks from the ring; the read offset moves on and the ring has room again.</summary>
    private int ConsumeOutput(uint* payload)
    {
        uint* context = Context(payload);

        if (context == null)
        {
            return HResult.InvalidArgument;
        }

        uint taken = payload[1];
        uint blocks = XmaContextLayout.OutputBlocks.Read(context);

        if (taken != 0 && blocks != 0)
        {
            uint read = XmaContextLayout.OutputReadOffset.Read(context);
            XmaContextLayout.OutputReadOffset.Write(context, (read + taken) % blocks);
            XmaContextLayout.OutputValid.Write(context, 1);
        }

        var decoder = _decoders[(int)*payload];
        _counters.XmaTaken();

        if (decoder is not null)
        {
            Service(decoder, context);
        }

        return HResult.Ok;
    }

    /// <summary>The context a message names by its first payload word, null for an index outside the array.</summary>
    private uint* Context(uint* payload)
    {
        if (payload == null || _contexts == 0 || *payload >= (uint)_decoders.Length)
        {
            return null;
        }

        return (uint*)(_contexts + ((nint)(*payload) * AcpHalLayout.XmaContextBytes));
    }

    /// <summary>Maps the context's buffers (APU addresses) and decodes into its ring.</summary>
    private void Service(XmaContextDecoder decoder, uint* context)
    {
        uint outputBytes = XmaContextLayout.OutputBlocks.Read(context) * XmaContextLayout.BlockBytes;
        nint output = _heap.ToVirtualAddress(XmaContextLayout.OutputBuffer.Read(context), outputBytes);

        if (output == 0)
        {
            _counters.XmaUnmapped();

            return;
        }

        var input0 = Input(XmaContextLayout.InputBuffer0.Read(context), XmaContextLayout.InputPackets0.Read(context));
        var input1 = Input(XmaContextLayout.InputBuffer1.Read(context), XmaContextLayout.InputPackets1.Read(context));
        int written = decoder.Service(context, input0, input1, new Span<byte>((void*)output, (int)outputBytes));

        if (written < 0)
        {
            _counters.XmaInputEnded();
            written = -1 - written;
        }

        _counters.XmaBlocksWritten(written);
    }

    private ReadOnlySpan<byte> Input(uint apuAddress, uint packets)
    {
        uint bytes = packets * XmaLayout.PacketBytes;
        nint address = _heap.ToVirtualAddress(apuAddress, bytes);

        if (address == 0)
        {
            return [];
        }

        return new ReadOnlySpan<byte>((void*)address, (int)bytes);
    }

    private void LogFirst(uint type, uint cookie, uint* payload)
    {
        lock (_gate)
        {
            if (!_loggedMessages.Add(type))
            {
                return;
            }
        }

        string word = payload == null ? "none" : $"0x{*payload:x}";
        _log.Info($"  [acphal] IAcpHal message {type} (cookie 0x{cookie:x}, payload {word}) (logged once per type)");
    }
}
