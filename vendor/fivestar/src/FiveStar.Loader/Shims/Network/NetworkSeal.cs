// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Net;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.Network;

/// <summary>
/// The record of blocked outbound attempts: every WinHTTP connection, Winsock connect, send or remote name lookup and
/// libHttpClient request fails inside the process, and each distinct (function, target) is logged on its first attempt.
/// Lookups of this computer's own name, of the empty name and of "localhost" are answered by Windows from the local
/// interfaces without a query (getaddrinfo, "Remarks"), so they go to the real ws2_32 functions.
/// </summary>
internal sealed class NetworkSeal
{
    private const string SystemWinsock = "ws2_32.dll";
    private const string Localhost = "localhost";

    private readonly ConcurrentDictionary<string, int> _attempts = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, bool> _allowed = new(StringComparer.Ordinal);
    private readonly LoaderLog _log;
    private readonly string _hostName;

    public NetworkSeal(LoaderLog log)
    {
        _log = log;
        _hostName = Dns.GetHostName();
        nint winsock = NativeLibrary.Load(Path.Combine(Environment.SystemDirectory, SystemWinsock));
        AnsiLookup = NativeLibrary.GetExport(winsock, "getaddrinfo");
        WideLookup = NativeLibrary.GetExport(winsock, "GetAddrInfoW");
    }

    /// <summary>Blocked attempts so far, by "function target".</summary>
    public IReadOnlyDictionary<string, int> Attempts => _attempts;

    /// <summary>The real ws2_32 getaddrinfo, for local names.</summary>
    public nint AnsiLookup { get; }

    /// <summary>The real ws2_32 GetAddrInfoW, for local names.</summary>
    public nint WideLookup { get; }

    /// <summary>True for names Windows answers from this computer alone: its host name, "" and "localhost".</summary>
    public bool IsLocalName(string? host)
    {
        if (host is null)
        {
            return false;
        }

        return host.Length == 0
            || host.Equals(_hostName, StringComparison.OrdinalIgnoreCase)
            || host.Equals(Localhost, StringComparison.OrdinalIgnoreCase);
    }

    public void Block(string api, string target)
    {
        string key = $"{api} {target}";
        int count = _attempts.AddOrUpdate(key, 1, (_, previous) => previous + 1);

        if (count == 1)
        {
            _log.Info($"  [network] blocked {key}");
        }
    }

    /// <summary>Logs a local lookup passed to Windows, once per name.</summary>
    public void Allow(string api, string target)
    {
        string key = $"{api} {target}";

        if (_allowed.TryAdd(key, true))
        {
            _log.Info($"  [network] allowed {key}");
        }
    }
}
