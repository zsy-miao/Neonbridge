// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>One generated stub: the import it stands for, what it returns and how often it was called.</summary>
internal sealed class UnresolvedStub
{
    private int _calls;

    public UnresolvedStub(string name, long returnValue, bool notImplemented)
    {
        Name = name;
        ReturnValue = returnValue;
        NotImplemented = notImplemented;
    }

    /// <summary>"module!symbol".</summary>
    public string Name { get; }

    public long ReturnValue { get; }

    /// <summary>A known function deliberately left unimplemented, as opposed to an import nothing resolved.</summary>
    public bool NotImplemented { get; }

    public int Calls => Volatile.Read(ref _calls);

    /// <summary>Counts a call and returns the new count.</summary>
    public int CountCall()
    {
        return Interlocked.Increment(ref _calls);
    }
}
