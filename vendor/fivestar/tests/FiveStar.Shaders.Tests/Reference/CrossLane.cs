// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>
/// Cross-lane semantics (RDNA ISA pseudo-code). Every function takes per-lane values of one modelled wave (index =
/// lane). <c>wave</c> is the AMD program wave size (32 or 64): it sets the address wrap of ds_(b)permute. When the
/// modelled wave is smaller than the AMD wave (WARP runs 4-lane waves), results that would come from lanes outside it
/// are null (undefined).
/// </summary>
public static class CrossLane
{
    /// <summary>DS_SWIZZLE_B32 QUAD_PERM: lane l reads lane (l &amp; ~3) | pattern[l &amp; 3].</summary>
    public static int QuadPermute(int lane, int[] pattern)
    {
        return (lane & ~3) | pattern[lane & 3];
    }

    /// <summary>DS_SWIZZLE_B32 BITMASK_PERM: five characters for bits 4..0: 0 force 0, 1 force 1, p preserve, i invert.</summary>
    public static int BitmaskPermute(int lane, string mask)
    {
        int and = 0;
        int or = 0;
        int xor = 0;

        for (var index = 0; index < 5; index++)
        {
            var bit = 1 << (4 - index);

            switch (mask[index])
            {
                case 'p':
                    and |= bit;
                    break;
                case 'i':
                    and |= bit;
                    xor |= bit;
                    break;
                case '1':
                    or |= bit;
                    break;
                default:
                    break;
            }
        }

        return (lane & ~31) | (((lane & 31 & and) | or) ^ xor);
    }

    /// <summary>DS_SWIZZLE_B32 SWAP n: lane l reads lane l ^ n.</summary>
    public static int Swap(int lane, int mask)
    {
        return lane ^ mask;
    }

    /// <summary>DS_SWIZZLE_B32 REVERSE n: reverse within groups of n lanes.</summary>
    public static int ReverseGroup(int lane, int group)
    {
        return lane ^ (group - 1);
    }

    /// <summary>DS_SWIZZLE_B32 BROADCAST n, k: lane k of each group of n lanes.</summary>
    public static int Broadcast(int lane, int group, int source)
    {
        return (lane & ~(group - 1)) | source;
    }

    /// <summary>Backward permute: lane i reads lane (addr[i] / 4) % wave; a disabled source lane reads 0; inactive lanes are null.</summary>
    public static uint?[] BackwardPermute(uint[] data, uint[] address, bool[] exec, int wave)
    {
        var result = new uint?[data.Length];

        for (var lane = 0; lane < data.Length; lane++)
        {
            if (!exec[lane])
            {
                continue;
            }

            var source = (int)(address[lane] / 4 % (uint)wave);

            if (source < data.Length)
            {
                result[lane] = exec[source] ? data[source] : 0;
            }
        }

        return result;
    }

    /// <summary>Forward permute: active lane i sends its value to lane (addr[i] / 4) % wave (highest source wins); unwritten lanes get 0.</summary>
    public static uint?[] ForwardPermute(uint[] data, uint[] address, bool[] exec, int wave)
    {
        var written = new uint[data.Length];
        var unknown = false;

        for (var lane = 0; lane < data.Length; lane++)
        {
            if (!exec[lane])
            {
                continue;
            }

            var target = (int)(address[lane] / 4 % (uint)wave);

            if (target >= data.Length)
            {
                unknown = true;
            }
            else
            {
                written[target] = data[lane];
            }
        }

        var result = new uint?[data.Length];

        for (var lane = 0; lane < data.Length; lane++)
        {
            if (exec[lane] && !unknown)
            {
                result[lane] = written[lane];
            }
        }

        return result;
    }

    /// <summary>V_PERMLANE16_B32 / V_PERMLANEX16_B32: lane l of row r reads nibble l of {hi, lo} in row r (x16: row r ^ 1).</summary>
    public static uint?[] Permlane16(uint[] data, uint selectLow, uint selectHigh, bool crossRow)
    {
        var select = ((ulong)selectHigh << 32) | selectLow;
        var result = new uint?[data.Length];
        var rowFlip = crossRow ? 1 : 0;

        for (var lane = 0; lane < data.Length; lane++)
        {
            var row = (lane >> 4) ^ rowFlip;
            var source = (row * 16) + (int)((select >> (4 * (lane & 15))) & 15);

            if (source < data.Length)
            {
                result[lane] = data[source];
            }
        }

        return result;
    }

    /// <summary>A lane mask as (low, high) dwords.</summary>
    public static (uint Low, uint High) Ballot(bool[] conditions)
    {
        ulong mask = 0;

        for (var lane = 0; lane < conditions.Length; lane++)
        {
            if (conditions[lane])
            {
                mask |= 1UL << lane;
            }
        }

        return ((uint)mask, (uint)(mask >> 32));
    }
}
