// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.Network;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The connectivity hint per <c>--network</c> mode, and the sealed WinHTTP and Winsock entry points.</summary>
public sealed unsafe class NetworkSealTests
{
    private const uint ImmediateDispatch = 3;
    private const int HostNotFound = 11001;
    private const int NetworkUnreachable = 10051;
    private const int NameNotResolved = 12007;
    private const int CannotConnect = 12029;
    private const ushort HttpsPort = 443;
    private const uint NoCallback = uint.MaxValue;
    private const int ConnectOrdinal = 4;
    private const int SockaddrInSize = 16;

    [Theory]
    [InlineData(false, null, 1u, NoCallback)]
    [InlineData(true, null, 3u, 3u)]
    [InlineData(true, 2u, 2u, 2u)]
    public void The_hint_reports_the_mode_and_sealed_online_runs_the_change_callback_once(bool sealedOnline, uint? level, uint expectedLevel, uint expectedCallback)
    {
        var mode = sealedOnline ? NetworkMode.SealedOnline : NetworkMode.Offline;
        using var test = TestRuntime.Create(network: new NetworkOptions(mode, (ConnectivityLevel?)level));
        byte* hint = stackalloc byte[16];
        int hintResult = ((delegate* unmanaged<byte*, int>)test.Gdk("XNetworkingGetConnectivityHint"))(hint);

        nint queue = 0;
        ((delegate* unmanaged<uint, uint, nint*, int>)test.Gdk("XTaskQueueCreate"))(ImmediateDispatch, ImmediateDispatch, &queue);
        uint seen = NoCallback;
        ulong token = 0;
        var register = (delegate* unmanaged<nint, nint, nint, ulong*, int>)test.Gdk("XNetworkingRegisterConnectivityHintChanged");
        int registered = register(queue, (nint)(&seen), (nint)(delegate* unmanaged<nint, byte*, void>)&RecordLevel, &token);

        Assert.Equal(0, hintResult);
        Assert.Equal(expectedLevel, *(uint*)hint);
        Assert.Equal(1, hint[12]);
        Assert.Equal(0, registered);
        Assert.Equal(expectedCallback, seen);
    }

    [Fact]
    public void Sealed_lookups_connects_and_requests_fail_inside_the_process()
    {
        using var test = TestRuntime.Create(network: new NetworkOptions(NetworkMode.SealedOnline, null));
        nint winsock = NativeLibrary.Load("ws2_32.dll");
        var getProcAddress = (delegate* unmanaged<nint, nint, nint>)test.Shim("kernel32.dll", "GetProcAddress");
        nint symbol = Marshal.StringToHGlobalAnsi("connect");
        nint byName = getProcAddress(winsock, symbol);
        Marshal.FreeHGlobal(symbol);

        Assert.Equal(test.Shim("ws2_32.dll", "connect"), byName);
        Assert.Equal(byName, getProcAddress(winsock, ConnectOrdinal));

        AssertLookupSealed(test);
        AssertLocalLookupAnswered(test, winsock);
        AssertConnectSealed(byName);
        AssertWinHttpSealed(test);

        var attempts = test.Runtime.Network.Attempts.Keys;
        Assert.Contains("getaddrinfo sealed.test:(null)", attempts);
        Assert.Contains("connect 192.0.2.1:443", attempts);
        Assert.Contains("WinHttpConnect sealed.test:443", attempts);
        NativeLibrary.Free(winsock);
    }

    private static void AssertLookupSealed(TestRuntime test)
    {
        var lookup = (delegate* unmanaged<byte*, byte*, nint, nint*, int>)test.Shim("ws2_32.dll", "getaddrinfo");
        nint result = 1;
        int found;

        fixed (byte* name = "sealed.test\0"u8)
        {
            found = lookup(name, null, 0, &result);
        }

        Assert.Equal(HostNotFound, found);
        Assert.Equal(HostNotFound, Marshal.GetLastSystemError());
        Assert.Equal(0, result);
    }

    /// <summary>"localhost" is answered by Windows from the local interfaces, so it reaches the real getaddrinfo.</summary>
    private static void AssertLocalLookupAnswered(TestRuntime test, nint winsock)
    {
        var lookup = (delegate* unmanaged<byte*, byte*, nint, nint*, int>)test.Shim("ws2_32.dll", "getaddrinfo");
        nint result = 0;
        int found;

        fixed (byte* name = "localhost\0"u8)
        {
            found = lookup(name, null, 0, &result);
        }

        Assert.Equal(0, found);
        Assert.NotEqual(0, result);
        ((delegate* unmanaged<nint, void>)NativeLibrary.GetExport(winsock, "freeaddrinfo"))(result);
    }

    /// <summary>A real socket handed to the sealed connect toward 192.0.2.1:443 (TEST-NET-1) stays unconnected.</summary>
    private static void AssertConnectSealed(nint connect)
    {
        using var socket = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp);
        byte* address = stackalloc byte[SockaddrInSize];
        new Span<byte>(address, SockaddrInSize).Clear();
        *(ushort*)address = (ushort)AddressFamily.InterNetwork;
        *(ushort*)(address + 2) = (ushort)IPAddress.HostToNetworkOrder((short)HttpsPort);
        IPAddress.Parse("192.0.2.1").GetAddressBytes().CopyTo(new Span<byte>(address + 4, 4));

        int connected = ((delegate* unmanaged<nint, byte*, int, int>)connect)(socket.Handle, address, SockaddrInSize);

        Assert.Equal(-1, connected);
        Assert.Equal(NetworkUnreachable, Marshal.GetLastSystemError());
        Assert.False(socket.Connected);
    }

    private static void AssertWinHttpSealed(TestRuntime test)
    {
        nint connect;

        fixed (char* server = "sealed.test")
        {
            connect = ((delegate* unmanaged<nint, char*, ushort, uint, nint>)test.Shim("winhttp.dll", "WinHttpConnect"))(1, server, HttpsPort, 0);
        }

        int connectError = Marshal.GetLastSystemError();
        var send = (delegate* unmanaged<nint, char*, uint, nint, uint, uint, nint, int>)test.Shim("winhttp.dll", "WinHttpSendRequest");
        int sent = send(1, null, 0, 0, 0, 0, 0);
        int sendError = Marshal.GetLastSystemError();

        Assert.Equal(0, connect);
        Assert.Equal(NameNotResolved, connectError);
        Assert.Equal(0, sent);
        Assert.Equal(CannotConnect, sendError);
    }

    [UnmanagedCallersOnly]
    private static void RecordLevel(nint context, byte* hint)
    {
        *(uint*)context = *(uint*)hint;
    }
}
