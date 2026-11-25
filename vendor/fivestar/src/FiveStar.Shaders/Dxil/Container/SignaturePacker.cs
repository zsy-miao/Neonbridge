// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// Assigns registers as dxc does (DxilSignatureAllocator, prefix-stable packing), so a stage the backend compiles links
/// with one dxc compiled: vertex inputs one row each in order, render targets at their index, and every other signature
/// first fit in declaration order. Elements of a row share its interpolation mode, and inside a row arbitrary values
/// come before system values, which come before system-generated ones.
/// </summary>
public static class SignaturePacker
{
    private const int Registers = 32;
    private const int Components = 4;
    private const int ClipCullRows = 2;

    private const byte Occupied = 1;
    private const byte ArbitraryFlag = 2;
    private const byte SystemValueFlag = 4;
    private const byte SystemGeneratedFlag = 8;
    private const byte ClipCullFlag = 16;

    public static void Pack(IReadOnlyList<DxilSignatureElement> elements, SignaturePoint point)
    {
        var packed = elements.Where(IsPacked).ToList();

        switch (point)
        {
            case SignaturePoint.VertexInput:
                var row = 0;

                foreach (var element in packed)
                {
                    (element.StartRow, element.StartColumn) = (row, 0);
                    row += element.Rows;
                }

                break;
            case SignaturePoint.PixelOutput:
                foreach (var element in packed)
                {
                    (element.StartRow, element.StartColumn) = (element.Index, 0);
                }

                break;
            default:
                PrefixStable(packed);
                break;
        }
    }

    private static bool IsPacked(DxilSignatureElement element)
    {
        return element.Interpretation is SemanticInterpretation.Arbitrary or SemanticInterpretation.SystemValue
            or SemanticInterpretation.SystemGenerated or SemanticInterpretation.ClipCull or SemanticInterpretation.Target;
    }

    private static void PrefixStable(List<DxilSignatureElement> elements)
    {
        var rows = new Row[Registers];
        var clipRows = new List<int>();
        var clipUsed = 0;

        foreach (var element in elements)
        {
            if (element.Rows != 1)
            {
                throw new DxilBackendException($"signature element {element.Name}{element.Index} spans {element.Rows} rows (arrays are not supported)");
            }

            if (element.Interpretation != SemanticInterpretation.ClipCull)
            {
                Place(rows, Flag(element), element.Interpolation, element.Columns, element);

                continue;
            }

            // Clip and cull distances share up to two whole rows of their own.
            if (clipRows.Count == 0 || element.Columns > Components - clipUsed)
            {
                if (clipRows.Count == ClipCullRows)
                {
                    throw new DxilBackendException("more than two rows of clip and cull distances");
                }

                clipRows.Add(Place(rows, ClipCullFlag, element.Interpolation, Components, element));
                clipUsed = 0;
            }

            (element.StartRow, element.StartColumn) = (clipRows[^1], clipUsed);
            clipUsed += element.Columns;
        }
    }

    /// <summary>Puts an element of <paramref name="columns"/> in the first row and column it fits; returns its row.</summary>
    private static int Place(Row[] rows, byte flags, byte interpolation, int columns, DxilSignatureElement element)
    {
        for (var row = 0; row < rows.Length; row++)
        {
            rows[row] ??= new Row();
            var register = rows[row];

            if (register.Interpolation != 0 && register.Interpolation != interpolation)
            {
                continue;
            }

            for (var column = 0; column + columns <= Components; column++)
            {
                if (!register.Fits(flags, column, columns))
                {
                    continue;
                }

                register.Place(flags, interpolation, column, columns);
                (element.StartRow, element.StartColumn) = (row, column);

                return row;
            }
        }

        throw new DxilBackendException($"signature element {element.Name}{element.Index} does not fit in {Registers} registers");
    }

    private static byte Flag(DxilSignatureElement element)
    {
        return element.Interpretation switch
        {
            SemanticInterpretation.SystemValue => SystemValueFlag,
            SemanticInterpretation.SystemGenerated => SystemGeneratedFlag,
            _ => ArbitraryFlag,
        };
    }

    /// <summary>Flags of the elements that may not go left of an element with <paramref name="flags"/>.</summary>
    private static byte ConflictsLeft(byte flags)
    {
        byte conflicts = 0;

        if ((flags & ArbitraryFlag) != 0)
        {
            conflicts |= SystemGeneratedFlag | SystemValueFlag | ClipCullFlag;
        }

        if ((flags & (SystemValueFlag | ClipCullFlag)) != 0)
        {
            conflicts |= SystemGeneratedFlag;
        }

        return conflicts;
    }

    /// <summary>Flags of the elements that may not go right of an element with <paramref name="flags"/>.</summary>
    private static byte ConflictsRight(byte flags)
    {
        byte conflicts = 0;

        if ((flags & SystemGeneratedFlag) != 0)
        {
            conflicts |= ArbitraryFlag | SystemValueFlag | ClipCullFlag;
        }

        if ((flags & (SystemValueFlag | ClipCullFlag)) != 0)
        {
            conflicts |= ArbitraryFlag;
        }

        return conflicts;
    }

    /// <summary>One register: its interpolation mode (0: none yet) and each component's flags.</summary>
    private sealed class Row
    {
        private readonly byte[] _flags = new byte[Components];

        public byte Interpolation { get; private set; }

        public bool Fits(byte flags, int column, int columns)
        {
            var mine = (byte)(flags | Occupied);

            for (var index = column; index < column + columns; index++)
            {
                if ((_flags[index] & mine) != 0)
                {
                    return false;
                }
            }

            return true;
        }

        public void Place(byte flags, byte interpolation, int column, int columns)
        {
            Interpolation = interpolation;
            var left = ConflictsLeft(flags);
            var right = ConflictsRight(flags);

            for (var index = 0; index < Components; index++)
            {
                if ((_flags[index] & Occupied) != 0)
                {
                    continue;
                }

                if (index < column)
                {
                    _flags[index] |= left;
                }
                else if (index < column + columns)
                {
                    _flags[index] = (byte)(Occupied | flags);
                }
                else
                {
                    _flags[index] |= right;
                }
            }
        }
    }
}
