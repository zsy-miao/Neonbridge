// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Compression;

/// <summary>
/// Inflates zlib (RFC 1950) and raw deflate (RFC 1951) streams with the acceptance rules of the reference zlib, and
/// reports exactly how many input bytes a complete stream used. Reusable; the output stays valid until the next call.
/// </summary>
public sealed class ZlibInflater
{
    private const int HeaderSize = 2;
    private const int TrailerSize = 4;
    private const int StoredLengthSize = 4;
    private const int BlockHeaderBits = 3;
    private const int BlockTypeStored = 0;
    private const int BlockTypeFixed = 1;
    private const int BlockTypeDynamic = 2;
    private const int BitsPerByte = 8;

    // RFC 1950 section 2.2: CMF low nibble = method 8 (deflate), high nibble = log2(window) - 8 at most 7,
    // FLG bit 5 = preset dictionary, and CMF * 256 + FLG a multiple of 31.
    private const int MethodMask = 0x0F;
    private const int MethodDeflate = 8;
    private const int WindowInfoShift = 4;
    private const int MaxWindowInfo = 7;
    private const int PresetDictionaryFlag = 0x20;
    private const int HeaderCheckDivisor = 31;

    private const int LiteralCountBits = 5;
    private const int DistanceCountBits = 5;
    private const int CodeLengthCountBits = 4;
    private const int MinCodeLengthCodes = 4;
    private const int StoredLengthMask = 0xFFFF;
    private const int InitialCapacity = 1 << 16;
    private const int GrowthFactor = 2;

    private readonly HuffmanCode _fixedLiterals = new(DeflateTables.FixedLiteralCodes);
    private readonly HuffmanCode _fixedDistances = new(DeflateTables.FixedDistanceCodes);
    private readonly HuffmanCode _literals = new(DeflateTables.MaxLiteralCodes);
    private readonly HuffmanCode _distances = new(DeflateTables.MaxDistanceCodes);
    private readonly HuffmanCode _codeLengthCode = new(DeflateTables.CodeLengthCodes);
    private readonly byte[] _lengths = new byte[DeflateTables.MaxLiteralCodes + DeflateTables.MaxDistanceCodes];
    private readonly byte[] _codeLengthLengths = new byte[DeflateTables.CodeLengthCodes];
    private byte[] _output = new byte[InitialCapacity];
    private int _length;
    private int _maxOutput;

    public ZlibInflater()
    {
        _fixedLiterals.Build(DeflateTables.FixedLiteralLengths, true);
        _fixedDistances.Build(DeflateTables.FixedDistanceLengths, true);
    }

    /// <summary>Gets the bytes produced by the last call (all decoded bytes, also when it did not complete).</summary>
    public ReadOnlySpan<byte> Output => _output.AsSpan(0, _length);

    /// <summary>Inflates one stream from the start of <paramref name="input"/>, producing at most <paramref name="maxOutput"/> bytes.</summary>
    public InflateResult Inflate(ReadOnlySpan<byte> input, DeflateWrapper wrapper, int maxOutput)
    {
        _length = 0;
        _maxOutput = maxOutput;
        var bits = new BitInput(input);

        if (wrapper == DeflateWrapper.Zlib)
        {
            if (input.Length < HeaderSize)
            {
                return new InflateResult(InflateStatus.Truncated, 0);
            }

            if (!HeaderIsValid(input[0], input[1]))
            {
                return new InflateResult(InflateStatus.Invalid, 0);
            }

            bits.Skip(HeaderSize);
        }

        var status = InflateBlocks(ref bits);

        if (status != InflateStatus.Complete)
        {
            return new InflateResult(status, 0);
        }

        bits.AlignToByte();
        bits.ReturnBufferedBytes();

        if (wrapper == DeflateWrapper.Raw)
        {
            return new InflateResult(InflateStatus.Complete, bits.Position);
        }

        return CheckTrailer(ref bits);
    }

    private static bool HeaderIsValid(byte cmf, byte flg)
    {
        if (((cmf << BitsPerByte) | flg) % HeaderCheckDivisor != 0)
        {
            return false;
        }

        if ((cmf & MethodMask) != MethodDeflate || cmf >> WindowInfoShift > MaxWindowInfo)
        {
            return false;
        }

        return (flg & PresetDictionaryFlag) == 0;
    }

    private InflateResult CheckTrailer(ref BitInput bits)
    {
        var rest = bits.Remaining;

        if (rest.Length < TrailerSize)
        {
            return new InflateResult(InflateStatus.Truncated, 0);
        }

        if (BinaryPrimitives.ReadUInt32BigEndian(rest) != Adler32.Compute(Output))
        {
            return new InflateResult(InflateStatus.Invalid, 0);
        }

        return new InflateResult(InflateStatus.Complete, bits.Position + TrailerSize);
    }

    // Complete here means "the final block ended"; any other status ends the stream.
    private InflateStatus InflateBlocks(ref BitInput bits)
    {
        while (true)
        {
            if (!bits.TryRead(BlockHeaderBits, out var header))
            {
                return InflateStatus.Truncated;
            }

            var status = InflateBlock(ref bits, header >> 1);

            if (status != InflateStatus.Complete || (header & 1) != 0)
            {
                return status;
            }
        }
    }

    private InflateStatus InflateBlock(ref BitInput bits, int type)
    {
        switch (type)
        {
            case BlockTypeStored:
                return CopyStored(ref bits);
            case BlockTypeFixed:
                return InflateCodes(ref bits, _fixedLiterals, _fixedDistances);
            case BlockTypeDynamic:
                return InflateDynamic(ref bits);
            default:
                return InflateStatus.Invalid;
        }
    }

    private InflateStatus CopyStored(ref BitInput bits)
    {
        bits.AlignToByte();
        bits.ReturnBufferedBytes();
        var rest = bits.Remaining;

        if (rest.Length < StoredLengthSize)
        {
            return InflateStatus.Truncated;
        }

        int length = BinaryPrimitives.ReadUInt16LittleEndian(rest);
        int complement = BinaryPrimitives.ReadUInt16LittleEndian(rest[sizeof(ushort)..]);

        if (length != (~complement & StoredLengthMask))
        {
            return InflateStatus.Invalid;
        }

        if (_length + length > _maxOutput)
        {
            return InflateStatus.OutputLimit;
        }

        var available = Math.Min(length, rest.Length - StoredLengthSize);
        EnsureCapacity(_length + available);
        rest.Slice(StoredLengthSize, available).CopyTo(_output.AsSpan(_length));
        _length += available;
        bits.Skip(StoredLengthSize + available);

        if (available < length)
        {
            return InflateStatus.Truncated;
        }

        return InflateStatus.Complete;
    }

    private InflateStatus InflateDynamic(ref BitInput bits)
    {
        const int HeaderBits = LiteralCountBits + DistanceCountBits + CodeLengthCountBits;

        if (!bits.TryRead(HeaderBits, out var header))
        {
            return InflateStatus.Truncated;
        }

        var literalCount = (header & ((1 << LiteralCountBits) - 1)) + DeflateTables.FirstLengthSymbol;
        var distanceCount = ((header >> LiteralCountBits) & ((1 << DistanceCountBits) - 1)) + 1;
        var codeLengthCount = (header >> (LiteralCountBits + DistanceCountBits)) + MinCodeLengthCodes;

        if (literalCount > DeflateTables.MaxLiteralCodes || distanceCount > DeflateTables.MaxDistanceCodes)
        {
            return InflateStatus.Invalid;
        }

        var status = ReadCodeLengthCode(ref bits, codeLengthCount);

        if (status != InflateStatus.Complete)
        {
            return status;
        }

        status = ReadCodeLengths(ref bits, literalCount + distanceCount);

        if (status != InflateStatus.Complete)
        {
            return status;
        }

        var lengths = _lengths.AsSpan(0, literalCount + distanceCount);

        if (lengths[DeflateTables.EndOfBlock] == 0
            || !_literals.Build(lengths[..literalCount], true)
            || !_distances.Build(lengths[literalCount..], true))
        {
            return InflateStatus.Invalid;
        }

        return InflateCodes(ref bits, _literals, _distances);
    }

    private InflateStatus ReadCodeLengthCode(ref BitInput bits, int count)
    {
        Array.Clear(_codeLengthLengths);

        for (var index = 0; index < count; index++)
        {
            if (!bits.TryRead(DeflateTables.CodeLengthBits, out var length))
            {
                return InflateStatus.Truncated;
            }

            _codeLengthLengths[DeflateTables.CodeLengthOrder[index]] = (byte)length;
        }

        var built = _codeLengthCode.Build(_codeLengthLengths);

        if (!built || _codeLengthLengths.AsSpan().IndexOfAnyExcept((byte)0) < 0)
        {
            return InflateStatus.Invalid;
        }

        return InflateStatus.Complete;
    }

    private InflateStatus ReadCodeLengths(ref BitInput bits, int total)
    {
        var filled = 0;

        while (filled < total)
        {
            var symbol = _codeLengthCode.Decode(ref bits);

            if (symbol < 0)
            {
                return DecodeFailure(symbol);
            }

            if (symbol < DeflateTables.RepeatPrevious)
            {
                _lengths[filled] = (byte)symbol;
                filled++;

                continue;
            }

            var status = ExpandRepeat(ref bits, symbol, total, ref filled);

            if (status != InflateStatus.Complete)
            {
                return status;
            }
        }

        return InflateStatus.Complete;
    }

    private InflateStatus ExpandRepeat(ref BitInput bits, int symbol, int total, ref int filled)
    {
        var (repeatBase, extraBits) = RepeatShape(symbol);

        if (!bits.TryRead(extraBits, out var extra))
        {
            return InflateStatus.Truncated;
        }

        byte value = 0;

        if (symbol == DeflateTables.RepeatPrevious)
        {
            if (filled == 0)
            {
                return InflateStatus.Invalid;
            }

            value = _lengths[filled - 1];
        }

        var repeat = repeatBase + extra;

        if (filled + repeat > total)
        {
            return InflateStatus.Invalid;
        }

        _lengths.AsSpan(filled, repeat).Fill(value);
        filled += repeat;

        return InflateStatus.Complete;
    }

    private static (int RepeatBase, int ExtraBits) RepeatShape(int symbol)
    {
        switch (symbol)
        {
            case DeflateTables.RepeatPrevious:
                return (DeflateTables.RepeatPreviousBase, DeflateTables.RepeatPreviousExtraBits);
            case DeflateTables.RepeatZeroShort:
                return (DeflateTables.RepeatZeroShortBase, DeflateTables.RepeatZeroShortExtraBits);
            default:
                return (DeflateTables.RepeatZeroLongBase, DeflateTables.RepeatZeroLongExtraBits);
        }
    }

    private InflateStatus InflateCodes(ref BitInput bits, HuffmanCode literals, HuffmanCode distances)
    {
        while (true)
        {
            var symbol = literals.Decode(ref bits);

            if (symbol < 0)
            {
                return DecodeFailure(symbol);
            }

            if (symbol < DeflateTables.EndOfBlock)
            {
                if (_length >= _maxOutput)
                {
                    return InflateStatus.OutputLimit;
                }

                EnsureCapacity(_length + 1);
                _output[_length] = (byte)symbol;
                _length++;

                continue;
            }

            if (symbol == DeflateTables.EndOfBlock)
            {
                return InflateStatus.Complete;
            }

            var status = CopyMatch(ref bits, symbol - DeflateTables.FirstLengthSymbol, distances);

            if (status != InflateStatus.Complete)
            {
                return status;
            }
        }
    }

    private InflateStatus CopyMatch(ref BitInput bits, int lengthSymbol, HuffmanCode distances)
    {
        if (lengthSymbol >= DeflateTables.LengthBase.Length)
        {
            return InflateStatus.Invalid;
        }

        if (!bits.TryRead(DeflateTables.LengthExtra[lengthSymbol], out var lengthExtra))
        {
            return InflateStatus.Truncated;
        }

        var distanceSymbol = distances.Decode(ref bits);

        if (distanceSymbol < 0)
        {
            return DecodeFailure(distanceSymbol);
        }

        if (distanceSymbol >= DeflateTables.DistanceBase.Length)
        {
            return InflateStatus.Invalid;
        }

        if (!bits.TryRead(DeflateTables.DistanceExtra[distanceSymbol], out var distanceExtra))
        {
            return InflateStatus.Truncated;
        }

        var length = DeflateTables.LengthBase[lengthSymbol] + lengthExtra;
        var distance = DeflateTables.DistanceBase[distanceSymbol] + distanceExtra;

        if (distance > _length)
        {
            return InflateStatus.Invalid;
        }

        if (_length + length > _maxOutput)
        {
            return InflateStatus.OutputLimit;
        }

        CopyBack(distance, length);

        return InflateStatus.Complete;
    }

    private void CopyBack(int distance, int length)
    {
        EnsureCapacity(_length + length);
        var output = _output;
        var from = _length - distance;

        if (distance >= length)
        {
            Buffer.BlockCopy(output, from, output, _length, length);
        }
        else
        {
            for (var index = 0; index < length; index++)
            {
                output[_length + index] = output[from + index];
            }
        }

        _length += length;
    }

    private static InflateStatus DecodeFailure(int symbol)
    {
        if (symbol == HuffmanCode.Truncated)
        {
            return InflateStatus.Truncated;
        }

        return InflateStatus.Invalid;
    }

    private void EnsureCapacity(int needed)
    {
        if (needed <= _output.Length)
        {
            return;
        }

        var capacity = Math.Max(needed, (int)Math.Min((long)_output.Length * GrowthFactor, Array.MaxLength));
        Array.Resize(ref _output, capacity);
    }
}
