// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>How an expected result dword is compared.</summary>
public enum ExpectedKind
{
    Exact,
    Ulp,
    Near,
    Nan,
    Any,
    Either,
}
