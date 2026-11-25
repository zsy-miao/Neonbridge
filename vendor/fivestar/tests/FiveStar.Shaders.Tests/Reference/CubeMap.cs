// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Reference;

/// <summary>V_CUBEID / V_CUBESC / V_CUBETC / V_CUBEMA (RDNA ISA pseudo-code; faces 0 +X, 1 -X, 2 +Y, 3 -Y, 4 +Z, 5 -Z).</summary>
public static class CubeMap
{
    public static double Face(double x, double y, double z)
    {
        if (ZMajor(x, y, z))
        {
            return Pick(z < 0, 5.0, 4.0);
        }

        if (YMajor(x, y))
        {
            return Pick(y < 0, 3.0, 2.0);
        }

        return Pick(x < 0, 1.0, 0.0);
    }

    public static double S(double x, double y, double z)
    {
        if (ZMajor(x, y, z))
        {
            return Pick(z < 0, -x, x);
        }

        if (YMajor(x, y))
        {
            return x;
        }

        return Pick(x < 0, z, -z);
    }

    public static double T(double x, double y, double z)
    {
        if (ZMajor(x, y, z))
        {
            return -y;
        }

        if (YMajor(x, y))
        {
            return Pick(y < 0, -z, z);
        }

        return -y;
    }

    public static double MajorAxis(double x, double y, double z)
    {
        if (ZMajor(x, y, z))
        {
            return 2.0 * z;
        }

        if (YMajor(x, y))
        {
            return 2.0 * y;
        }

        return 2.0 * x;
    }

    private static bool ZMajor(double x, double y, double z)
    {
        return Math.Abs(z) >= Math.Abs(x) && Math.Abs(z) >= Math.Abs(y);
    }

    private static bool YMajor(double x, double y)
    {
        return Math.Abs(y) >= Math.Abs(x);
    }

    private static double Pick(bool condition, double whenTrue, double whenFalse)
    {
        if (condition)
        {
            return whenTrue;
        }

        return whenFalse;
    }
}
