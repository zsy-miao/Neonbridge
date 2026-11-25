// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using System.Text;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The offline answers of XGameUI, XNetworking, XUser, XDisplay and feature availability, through their native entry points.</summary>
public sealed unsafe class PlatformServicesTests
{
    private const int Abort = unchecked((int)0x80004004);
    private const int NotFound = unchecked((int)0x80070490);
    private const uint CancelButton = 2;

    [Fact]
    public void Dialogs_complete_as_dismissed_through_XAsync()
    {
        using var test = TestRuntime.Create();
        var status = (delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus");
        nint dialog = TestNative.AsyncBlock(0, 0);
        nint text = TestNative.AsyncBlock(0, 0);

        var showDialog = (delegate* unmanaged<nint, nint, nint, nint, nint, nint, uint, uint, int>)test.Gdk("XGameUiShowMessageDialogAsync");
        int begun = showDialog(dialog, 0, 0, 0, 0, 0, 0, CancelButton);
        int dialogStatus = status(dialog, 1);
        uint button = 0;
        int dialogResult = ((delegate* unmanaged<nint, uint*, int>)test.Gdk("XGameUiShowMessageDialogResult"))(dialog, &button);

        ((delegate* unmanaged<nint, int>)test.Gdk("XGameUiShowTextEntryAsync"))(text);
        int textStatus = status(text, 1);
        uint size = 0;
        int textResult = ((delegate* unmanaged<nint, uint*, int>)test.Gdk("XGameUiShowTextEntryResultSize"))(text, &size);

        Assert.Equal(0, begun);
        Assert.Equal(0, dialogStatus);
        Assert.Equal(0, dialogResult);
        Assert.Equal(CancelButton, button);
        Assert.Equal(Abort, textStatus);
        Assert.Equal(Abort, textResult);
    }

    [Fact]
    public void Networking_reports_no_connectivity_and_the_xbox_port()
    {
        using var test = TestRuntime.Create();
        byte* hint = stackalloc byte[16];
        int hintResult = ((delegate* unmanaged<byte*, int>)test.Gdk("XNetworkingGetConnectivityHint"))(hint);

        nint block = TestNative.AsyncBlock(0, 0);
        ((delegate* unmanaged<nint, int>)test.Gdk("XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync"))(block);
        ((delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus"))(block, 1);
        ushort port = 0;
        int portResult = ((delegate* unmanaged<nint, ushort*, int>)test.Gdk("XNetworkingQueryPreferredLocalUdpMultiplayerPortAsyncResult"))(block, &port);

        ulong token = 0;
        var register = (delegate* unmanaged<nint, nint, nint, ulong*, int>)test.Gdk("XNetworkingRegisterConnectivityHintChanged");
        var unregister = (delegate* unmanaged<ulong, byte, byte>)test.Gdk("XNetworkingUnregisterConnectivityHintChanged");
        int registered = register(0, 0, 0x1234, &token);
        byte first = unregister(token, 1);
        byte second = unregister(token, 1);

        Assert.Equal(0, hintResult);
        Assert.Equal(1u, *(uint*)hint);
        Assert.Equal(1, hint[12]);
        Assert.Equal(0, portResult);
        Assert.Equal(3074, port);
        Assert.Equal(0, registered);

        Assert.Equal(1, first);
        Assert.Equal(0, second);
    }

    [Fact]
    public void Controller_display_and_features_answer_from_the_pc()
    {
        using var test = TestRuntime.Create();
        nint block = TestNative.AsyncBlock(0, 0);
        nint user = test.Runtime.Gdk.Users.Reference();
        byte* device = stackalloc byte[32];

        ((delegate* unmanaged<nint, nint, int>)test.Gdk("XUserFindControllerForUserWithUiAsync"))(user, block);
        ((delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus"))(block, 1);
        int paired = ((delegate* unmanaged<nint, byte*, int>)test.Gdk("XUserFindControllerForUserWithUiResult"))(block, device);

        float* info = stackalloc float[3];
        uint hdr = ((delegate* unmanaged<uint, float*, uint>)test.Gdk("XDisplayTryEnableHdrMode"))(0, info);
        nint deferral = 0;
        ((delegate* unmanaged<nint*, int>)test.Gdk("XDisplayAcquireTimeoutDeferral"))(&deferral);
        int held = test.Runtime.Gdk.Display.ActiveDeferrals;
        ((delegate* unmanaged<nint, void>)test.Gdk("XDisplayCloseTimeoutDeferralHandle"))(deferral);

        var feature = (delegate* unmanaged<uint, byte>)test.Gdk("XGameRuntimeIsFeatureAvailable");

        // The default render endpoint: an MMDevice id such as "{0.0.0.00000000}.{guid}", or E_NOTFOUND without audio.
        char* endpoint = stackalloc char[256];
        nuint used = 0;
        int audio = ((delegate* unmanaged<ulong, uint, nuint, char*, nuint*, int>)test.Gdk("XUserGetDefaultAudioEndpointUtf16"))(1, 0, 256, endpoint, &used);

        Assert.True(audio == 0 || audio == NotFound, $"0x{audio:x8}");
        Assert.True(audio != 0 || (endpoint[0] == '{' && (int)used == new string(endpoint).Length + 1));

        Assert.Equal(0, paired);
        Assert.Equal(SHA256.HashData(Encoding.UTF8.GetBytes("XInput gamepad 1")), new ReadOnlySpan<byte>(device, 32).ToArray());
        Assert.InRange(hdr, 1u, 2u);
        Assert.Equal(hdr == 1, info[1] > 0);
        Assert.Equal(1, held);
        Assert.Equal(0, test.Runtime.Gdk.Display.ActiveDeferrals);

        Assert.Equal(0, feature(1));
        Assert.Equal(1, feature(18));
    }
}
