// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Network;

/// <summary>
/// Winsock sealed: name lookups fail with WSAHOST_NOT_FOUND (getaddrinfo and GetAddrInfoW of the local names of
/// <see cref="NetworkSeal"/> excepted), connects and datagram sends with WSAENETUNREACH, listening with WSAENETDOWN,
/// and WSAIoctl (the way to ConnectEx and WSASendMsg) with WSAEOPNOTSUPP. Sockets can still be created, bound and
/// closed. The executable imports most of ws2_32.dll by ordinal, so the ordinals of
/// connect (4), listen (13), sendto (20), gethostbyaddr (51) and gethostbyname (52) are bound too.
/// </summary>
internal static unsafe partial class WinsockShim
{
    private const string Module = "ws2_32.dll";

    [Shim(Module, "getaddrinfo", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int GetAddrInfo(byte* node, byte* service, nint hints, nint* result)
    {
        try
        {
            var seal = ShimContext.Runtime.Network;
            string? host = SocketAddressText.Ansi(node);
            string target = SocketAddressText.Lookup(host, SocketAddressText.Ansi(service));

            if (!seal.IsLocalName(host))
            {
                return NotFound("getaddrinfo", target, result);
            }

            seal.Allow("getaddrinfo", target);

            return Local(((delegate* unmanaged<byte*, byte*, nint, nint*, int>)seal.AnsiLookup)(node, service, hints, result));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.GetAddrInfo");

            return HResult.Fail;
        }
    }

    [Shim(Module, "GetAddrInfoW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int GetAddrInfoW(char* node, char* service, nint hints, nint* result)
    {
        try
        {
            var seal = ShimContext.Runtime.Network;
            string? host = SocketAddressText.Wide(node);
            string target = SocketAddressText.Lookup(host, SocketAddressText.Wide(service));

            if (!seal.IsLocalName(host))
            {
                return NotFound("GetAddrInfoW", target, result);
            }

            seal.Allow("GetAddrInfoW", target);

            return Local(((delegate* unmanaged<char*, char*, nint, nint*, int>)seal.WideLookup)(node, service, hints, result));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.GetAddrInfoW");

            return HResult.Fail;
        }
    }

    /// <summary>GetAddrInfoExW(name, service, namespace, nspId, hints, result, timeout, overlapped, completion, handle).</summary>
    [Shim(Module, "GetAddrInfoExW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int GetAddrInfoExW(char* name, char* service, uint space, nint provider, nint hints, nint* result, nint timeout, nint overlapped, nint completion, nint handle)
    {
        try
        {
            return NotFound("GetAddrInfoExW", SocketAddressText.Lookup(SocketAddressText.Wide(name), SocketAddressText.Wide(service)), result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.GetAddrInfoExW");

            return HResult.Fail;
        }
    }

    [Shim(Module, "GetAddrInfoExA", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int GetAddrInfoExA(byte* name, byte* service, uint space, nint provider, nint hints, nint* result, nint timeout, nint overlapped, nint completion, nint handle)
    {
        try
        {
            return NotFound("GetAddrInfoExA", SocketAddressText.Lookup(SocketAddressText.Ansi(name), SocketAddressText.Ansi(service)), result);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.GetAddrInfoExA");

            return HResult.Fail;
        }
    }

    [Shim(Module, "gethostbyname", SetsLastError = true)]
    [Shim(Module, "#52")]
    [UnmanagedCallersOnly]
    private static nint GetHostByName(byte* name)
    {
        try
        {
            return Fail("gethostbyname", SocketAddressText.Ansi(name) ?? "(null)", NetworkErrors.HostNotFound);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.GetHostByName");

            return default;
        }
    }

    [Shim(Module, "gethostbyaddr", SetsLastError = true)]
    [Shim(Module, "#51")]
    [UnmanagedCallersOnly]
    private static nint GetHostByAddress(byte* address, int length, int type)
    {
        try
        {
            return Fail("gethostbyaddr", "(address)", NetworkErrors.HostNotFound);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.GetHostByAddress");

            return default;
        }
    }

    [Shim(Module, "connect", SetsLastError = true)]
    [Shim(Module, "#4")]
    [UnmanagedCallersOnly]
    private static int Connect(nint socket, byte* name, int length)
    {
        try
        {
            return (int)Fail("connect", SocketAddressText.Format(name, length), NetworkErrors.NetworkUnreachable, NetworkErrors.SocketError);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.Connect");

            return HResult.Fail;
        }
    }

    /// <summary>WSAConnect(s, name, namelen, callerData, calleeData, sqos, gqos).</summary>
    [Shim(Module, "WSAConnect", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int WsaConnect(nint socket, byte* name, int length, nint callerData, nint calleeData, nint sqos, nint gqos)
    {
        try
        {
            return (int)Fail("WSAConnect", SocketAddressText.Format(name, length), NetworkErrors.NetworkUnreachable, NetworkErrors.SocketError);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.WsaConnect");

            return HResult.Fail;
        }
    }

    /// <summary>BOOL WSAConnectByNameW(s, nodeName, serviceName, localLength*, local, remoteLength*, remote, timeout, reserved).</summary>
    [Shim(Module, "WSAConnectByNameW", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int WsaConnectByNameW(nint socket, char* node, char* service, nint localLength, nint local, nint remoteLength, nint remote, nint timeout, nint reserved)
    {
        try
        {
            return (int)Fail("WSAConnectByNameW", SocketAddressText.Lookup(SocketAddressText.Wide(node), SocketAddressText.Wide(service)), NetworkErrors.HostNotFound);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.WsaConnectByNameW");

            return HResult.Fail;
        }
    }

    [Shim(Module, "WSAConnectByNameA", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int WsaConnectByNameA(nint socket, byte* node, byte* service, nint localLength, nint local, nint remoteLength, nint remote, nint timeout, nint reserved)
    {
        try
        {
            return (int)Fail("WSAConnectByNameA", SocketAddressText.Lookup(SocketAddressText.Ansi(node), SocketAddressText.Ansi(service)), NetworkErrors.HostNotFound);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.WsaConnectByNameA");

            return HResult.Fail;
        }
    }

    /// <summary>BOOL WSAConnectByList(s, addressList, localLength*, local, remoteLength*, remote, timeout, reserved).</summary>
    [Shim(Module, "WSAConnectByList", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int WsaConnectByList(nint socket, nint list, nint localLength, nint local, nint remoteLength, nint remote, nint timeout, nint reserved)
    {
        try
        {
            return (int)Fail("WSAConnectByList", "(address list)", NetworkErrors.NetworkUnreachable);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.WsaConnectByList");

            return HResult.Fail;
        }
    }

    [Shim(Module, "sendto", SetsLastError = true)]
    [Shim(Module, "#20")]
    [UnmanagedCallersOnly]
    private static int SendTo(nint socket, nint buffer, int length, int flags, byte* to, int toLength)
    {
        try
        {
            return (int)Fail("sendto", SocketAddressText.Format(to, toLength), NetworkErrors.NetworkUnreachable, NetworkErrors.SocketError);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.SendTo");

            return HResult.Fail;
        }
    }

    /// <summary>WSASendTo(s, buffers, count, sent*, flags, to, toLength, overlapped, completion).</summary>
    [Shim(Module, "WSASendTo", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int WsaSendTo(nint socket, nint buffers, uint count, nint sent, uint flags, byte* to, int toLength, nint overlapped, nint completion)
    {
        try
        {
            return (int)Fail("WSASendTo", SocketAddressText.Format(to, toLength), NetworkErrors.NetworkUnreachable, NetworkErrors.SocketError);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.WsaSendTo");

            return HResult.Fail;
        }
    }

    [Shim(Module, "listen", SetsLastError = true)]
    [Shim(Module, "#13")]
    [UnmanagedCallersOnly]
    private static int Listen(nint socket, int backlog)
    {
        try
        {
            return (int)Fail("listen", $"socket 0x{socket:x}", NetworkErrors.NetworkDown, NetworkErrors.SocketError);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.Listen");

            return HResult.Fail;
        }
    }

    /// <summary>WSAIoctl(s, code, in, inLength, out, outLength, returned*, overlapped, completion).</summary>
    [Shim(Module, "WSAIoctl", SetsLastError = true)]
    [UnmanagedCallersOnly]
    private static int WsaIoctl(nint socket, uint code, nint input, uint inputLength, nint output, uint outputLength, nint returned, nint overlapped, nint completion)
    {
        try
        {
            return (int)Fail("WSAIoctl", $"code 0x{code:x8}", NetworkErrors.OperationNotSupported, NetworkErrors.SocketError);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "WinsockShim.WsaIoctl");

            return HResult.Fail;
        }
    }

    /// <summary>The result of a local lookup Windows answered, also as the last error (the shim's return path resets it).</summary>
    private static int Local(int error)
    {
        ShimContext.Runtime.LastError.Set(error);

        return error;
    }

    /// <summary>A failed lookup: *result = NULL, the error returned and set as the last error.</summary>
    private static int NotFound(string api, string target, nint* result)
    {
        if (result != null)
        {
            *result = 0;
        }

        _ = Fail(api, target, NetworkErrors.HostNotFound);

        return NetworkErrors.HostNotFound;
    }

    private static nint Fail(string api, string target, int error, nint returnValue = 0)
    {
        ShimContext.Runtime.Network.Block(api, target);
        ShimContext.Runtime.LastError.Set(error);

        return returnValue;
    }
}
