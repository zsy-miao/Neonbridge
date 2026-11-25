// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

// Each lane is the decoder module of Microsoft silicon-rans mdu_minirans.v (MIT), following the bcpack-rs port
// (src/rans.rs, MIT); see THIRD_PARTY_NOTICES.md.

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The registers of a codec's 12 or 16 lanes, as arrays of 16 slots per field so the vector engines load them directly
/// (slots past the codec's lanes stay inert: never started, granted, pushed or written). Flags are 0 or all ones. Also
/// runs the rare steps of a lane's control: start-up, the fragment header bytes and the end of a fragment, which every
/// engine shares.
/// </summary>
internal unsafe struct LaneRegisters
{
    public const int Width = 16;

    /// <summary>The rANS lower bound L: the state renormalises byte by byte while below it.</summary>
    public const uint LowerBound = 1u << 24;

    public const int ByteShift = 8;

    public const uint Idle = 0;

    public const uint Header0 = 1;

    public const uint Header1 = 2;

    public const uint Header2 = 3;

    public const uint Processing = 4;

    public const uint Draining = 5;

    public const int Bc1Segments = RansModels.Bc1Thresholds + 1;

    // The hardware's alphabet_bits input is the lane's symbol width - 1.
    private const uint WidthOverAlphabet = 1;

    // Header byte 0: bits 0-3 the model q, bit 4 flush, bit 5 a three-byte header, bits 6-7 narrow the alphabet.
    private const uint ModelMask = 0xF;
    private const int FlushBit = 4;
    private const int LongHeaderBit = 5;
    private const int NarrowShift = 6;
    private const uint AlphabetMask = 0xF;
    private const uint MaxAlphabet = MduModelTable.AlphabetIndices - 1;
    private const uint ByteMask = 0xFF;

    // A three-byte header's length is (byte 2 << 8 | byte 1) + 257; a two-byte header's is byte 1 + 1.
    private const uint LongLengthBias = (1u << ByteShift) + 1;

    // Read by the vector engines.
    public fixed uint RansState[Width];
    public fixed uint InputLeft[Width];
    public fixed uint SymbolsLeft[Width];
    public fixed uint Symbol[Width];
    public fixed uint InputByte[Width];
    public fixed uint InputFull[Width];
    public fixed uint SymbolFull[Width];
    public fixed uint SegmentBase[Width];
    public fixed uint SymbolBase[Width];

    /// <summary>The symbol vector: the last symbol each lane handed over.</summary>
    public fixed uint Released[Width];

    /// <summary><c>[k * 16 + lane]</c>: the threshold of segment k + 1 in the lane's model.</summary>
    public fixed uint ThresholdRows[RansModels.Bc1Thresholds * Width];

    /// <summary><c>[segment * 16 + lane]</c>: the lane model's rows of <see cref="RansModels.SegA"/>.</summary>
    public fixed uint SegARows[Bc1Segments * Width];

    /// <summary><c>[segment * 16 + lane]</c>: the lane model's rows of <see cref="RansModels.SegB"/>.</summary>
    public fixed uint SegBRows[Bc1Segments * Width];

    /// <summary><c>[segment * 16 + lane]</c>: the lane model's rows of <see cref="RansModels.ForwardSegment"/>.</summary>
    public fixed uint ForwardRows[Bc1Segments * Width];

    // Read by the shared control steps only.
    public fixed uint Control[Width];
    public fixed uint AlphabetBits[Width];
    public fixed uint Model[Width];
    public fixed uint Alphabet[Width];
    public fixed uint LongHeader[Width];
    public fixed uint Flush[Width];
    public fixed uint Go[Width];
    public fixed uint FinalState[Width];

    /// <summary>The lanes in PROCESSING, in DRAINING and in a header state, as bit masks.</summary>
    public uint ProcessingLanes;
    public uint DrainingLanes;
    public uint HeaderLanes;

    /// <summary>The sum of all control states (part of the progress test) and the number of finished lanes.</summary>
    public int ControlSum;
    public int FinishedLanes;

    /// <summary>The codec's lanes, and the bit mask of them.</summary>
    public int Lanes;
    public uint AllLanes;

    /// <summary>Resets to the power-on state for lanes of the given symbol widths: every lane IDLE with go set, model (0, 0).</summary>
    public void Reset(ReadOnlySpan<byte> widths)
    {
        fixed (LaneRegisters* self = &this)
        {
            new Span<byte>(self, sizeof(LaneRegisters)).Clear();
        }

        Lanes = widths.Length;
        AllLanes = (uint)((1L << Lanes) - 1);

        for (var lane = 0; lane < Lanes; lane++)
        {
            AlphabetBits[lane] = widths[lane] - WidthOverAlphabet;
            Go[lane] = 1;
        }

        for (var lane = 0; lane < Width; lane++)
        {
            SetModel(lane, 0);
        }
    }

    /// <summary>Whether all lanes finished and hold no symbol.</summary>
    [HotPath]
    public readonly bool Finished()
    {
        if (FinishedLanes != Lanes)
        {
            return false;
        }

        for (var lane = 0; lane < Lanes; lane++)
        {
            if (SymbolFull[lane] != 0)
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>The bit mask of lanes whose final state is not L, L / 2^8 or L / 2^16.</summary>
    public readonly int IllegalEndStates()
    {
        var bad = 0;

        for (var lane = 0; lane < Lanes; lane++)
        {
            var x = FinalState[lane];

            if (x != LowerBound && x != LowerBound >> ByteShift && x != LowerBound >> ByteShift >> ByteShift)
            {
                bad |= 1 << lane;
            }
        }

        return bad;
    }

    /// <summary>
    /// The end of a fragment's input (PROCESSING with no input left) or of draining (the state reached L or below),
    /// after the clock's registers are written: the lane finishes, drains or reads the next header.
    /// </summary>
    [HotPath]
    public void Transition(int lane)
    {
        var x = RansState[lane];

        if (SymbolsLeft[lane] == 0)
        {
            FinalState[lane] = x;

            if (SymbolFull[lane] != 0)
            {
                FinalState[lane] = RansModels.Advance(SymbolBase[lane], x, Symbol[lane]);
            }

            FinishedLanes++;
            SetControl(lane, Idle);
        }
        else if (Control[lane] == Processing && x != LowerBound)
        {
            SetControl(lane, Draining);
        }
        else
        {
            if (Flush[lane] != 0)
            {
                RansState[lane] = 0;
            }

            SetControl(lane, Header0);
        }
    }

    /// <summary>Phase 1 of a lane in IDLE or a header state. False when a header selects an undefined alphabet.</summary>
    [HotPath]
    public bool Header(int lane, uint target)
    {
        switch (Control[lane])
        {
            case Idle:
                Start(lane, target);

                return true;
            case Header0:
                return ReadHeader0(lane);
            case Header1:
                ReadHeader1(lane);

                return true;
            case Header2:
                ReadHeader2(lane);

                return true;
            default:
                return true;
        }
    }

    private void Start(int lane, uint target)
    {
        if (Go[lane] == 0)
        {
            return;
        }

        Go[lane] = 0;
        SymbolsLeft[lane] = target;
        RansState[lane] = 0;
        SetControl(lane, Header0);
    }

    private bool ReadHeader0(int lane)
    {
        if (InputFull[lane] == 0)
        {
            return true;
        }

        var value = InputByte[lane];
        InputFull[lane] = 0;
        InputLeft[lane] = 0;
        Model[lane] = value & ModelMask;
        LongHeader[lane] = (value >> LongHeaderBit) & 1;
        Flush[lane] = (value >> FlushBit) & 1;

        // An index past the table reads as 9, which no model defines.
        Alphabet[lane] = Math.Min((AlphabetBits[lane] - (value >> NarrowShift)) & AlphabetMask, MaxAlphabet + 1);
        SetControl(lane, Header1);

        return Alphabet[lane] <= MaxAlphabet;
    }

    private void ReadHeader1(int lane)
    {
        if (InputFull[lane] == 0)
        {
            return;
        }

        InputFull[lane] = 0;
        SetModel(lane, RansModels.Index((int)Alphabet[lane], (int)Model[lane]));

        if (LongHeader[lane] != 0)
        {
            InputLeft[lane] = InputByte[lane];
            SetControl(lane, Header2);
        }
        else
        {
            InputLeft[lane] = InputByte[lane] + 1;
            SetControl(lane, Processing);
        }
    }

    private void ReadHeader2(int lane)
    {
        if (InputFull[lane] == 0)
        {
            return;
        }

        InputFull[lane] = 0;
        InputLeft[lane] = ((InputByte[lane] << ByteShift) | (InputLeft[lane] & ByteMask)) + LongLengthBias;
        SetControl(lane, Processing);
    }

    private void SetModel(int lane, int model)
    {
        var segments = (uint)(model * RansModels.SegmentStride);
        SegmentBase[lane] = segments;
        SymbolBase[lane] = (uint)(model * RansModels.SymbolStride);

        for (var k = 0; k < RansModels.Bc1Thresholds; k++)
        {
            ThresholdRows[(k * Width) + lane] = RansModels.Thresholds[segments + k];
        }

        for (var segment = 0; segment < Bc1Segments; segment++)
        {
            SegARows[(segment * Width) + lane] = RansModels.SegA[segments + segment];
            SegBRows[(segment * Width) + lane] = RansModels.SegB[segments + segment];
            ForwardRows[(segment * Width) + lane] = RansModels.ForwardSegment[segments + segment];
        }
    }

    private void SetControl(int lane, uint control)
    {
        ControlSum += (int)control - (int)Control[lane];
        Control[lane] = control;
        var bit = 1u << lane;
        ProcessingLanes = WithBit(ProcessingLanes, bit, control == Processing);
        DrainingLanes = WithBit(DrainingLanes, bit, control == Draining);
        HeaderLanes = WithBit(HeaderLanes, bit, control >= Header0 && control <= Header2);
    }

    private static uint WithBit(uint mask, uint bit, bool set)
    {
        if (set)
        {
            return mask | bit;
        }

        return mask & ~bit;
    }
}
