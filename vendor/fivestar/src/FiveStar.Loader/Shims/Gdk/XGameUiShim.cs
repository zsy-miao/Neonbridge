// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XGameUI.h: there is no system UI offline, so every dialog completes as if the player closed it at once. Dialogs
/// that only inform (achievements, profile card, invites, errors, state share) complete with S_OK; the message dialog
/// answers its cancel button; the player picker picks nobody; text entry and web sign-in complete with E_ABORT, the
/// result of a canceled dialog. The async block is the first argument of every *Async function, so the shims ignore
/// the arguments after the ones they read.
/// </summary>
internal static unsafe partial class XGameUiShim
{
    private const string MessageDialogApi = "XGameUiShowMessageDialogAsync";
    private const string ErrorDialogApi = "XGameUiShowErrorDialogAsync";
    private const string AchievementsApi = "XGameUiShowAchievementsAsync";
    private const string ProfileCardApi = "XGameUiShowPlayerProfileCardAsync";
    private const string SendInviteApi = "XGameUiShowSendGameInviteAsync";
    private const string ActivityInviteApi = "XGameUiShowMultiplayerActivityGameInviteAsync";
    private const string StateShareApi = "XGameUiShowStateShareAsync";
    private const string PlayerPickerApi = "XGameUiShowPlayerPickerAsync";
    private const string TextEntryApi = "XGameUiShowTextEntryAsync";
    private const string WebAuthenticationApi = "XGameUiShowWebAuthenticationAsync";

    /// <summary>HRESULT XGameUiShowMessageDialogAsync(XAsyncBlock*, title, content, three button texts, defaultButton, cancelButton).</summary>
    [Shim(GdkModule.Name, MessageDialogApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowMessageDialogAsync(nint async, nint title, nint content, nint first, nint second, nint third, uint defaultButton, uint cancelButton)
    {
        try
        {
            return Begin(MessageDialogApi, async, DismissedUiOperation.WithValue(cancelButton));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowMessageDialogAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameUiShowMessageDialogResult(XAsyncBlock*, XGameUiMessageDialogButton* resultButton).</summary>
    [Shim(GdkModule.Name, "XGameUiShowMessageDialogResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowMessageDialogResult(nint async, uint* button)
    {
        try
        {
            if (button == null)
            {
                return HResult.Pointer;
            }

            return Result(MessageDialogApi, async, sizeof(uint), (nint)button);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowMessageDialogResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, ErrorDialogApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowErrorDialogAsync(nint async)
    {
        try
        {
            return Begin(ErrorDialogApi, async, DismissedUiOperation.Closed());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowErrorDialogAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowErrorDialogResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowErrorDialogResult(nint async)
    {
        try
        {
            return Result(ErrorDialogApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowErrorDialogResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, AchievementsApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowAchievementsAsync(nint async)
    {
        try
        {
            return Begin(AchievementsApi, async, DismissedUiOperation.Closed());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowAchievementsAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowAchievementsResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowAchievementsResult(nint async)
    {
        try
        {
            return Result(AchievementsApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowAchievementsResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, ProfileCardApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowPlayerProfileCardAsync(nint async)
    {
        try
        {
            return Begin(ProfileCardApi, async, DismissedUiOperation.Closed());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowPlayerProfileCardAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowPlayerProfileCardResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowPlayerProfileCardResult(nint async)
    {
        try
        {
            return Result(ProfileCardApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowPlayerProfileCardResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, SendInviteApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowSendGameInviteAsync(nint async)
    {
        try
        {
            return Begin(SendInviteApi, async, DismissedUiOperation.Closed());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowSendGameInviteAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowSendGameInviteResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowSendGameInviteResult(nint async)
    {
        try
        {
            return Result(SendInviteApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowSendGameInviteResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, ActivityInviteApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowMultiplayerActivityGameInviteAsync(nint async)
    {
        try
        {
            return Begin(ActivityInviteApi, async, DismissedUiOperation.Closed());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowMultiplayerActivityGameInviteAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowMultiplayerActivityGameInviteResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowMultiplayerActivityGameInviteResult(nint async)
    {
        try
        {
            return Result(ActivityInviteApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowMultiplayerActivityGameInviteResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, StateShareApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowStateShareAsync(nint async)
    {
        try
        {
            return Begin(StateShareApi, async, DismissedUiOperation.Closed());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowStateShareAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowStateShareResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowStateShareResult(nint async)
    {
        try
        {
            return Result(StateShareApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowStateShareResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, PlayerPickerApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowPlayerPickerAsync(nint async)
    {
        try
        {
            return Begin(PlayerPickerApi, async, DismissedUiOperation.Closed());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowPlayerPickerAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameUiShowPlayerPickerResultCount(XAsyncBlock*, uint32_t* resultPlayersCount): nobody was picked.</summary>
    [Shim(GdkModule.Name, "XGameUiShowPlayerPickerResultCount")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowPlayerPickerResultCount(nint async, uint* count)
    {
        try
        {
            return NoPlayers(async, count);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowPlayerPickerResultCount");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameUiShowPlayerPickerResult(XAsyncBlock*, uint32_t count, uint64_t* players, uint32_t* resultPlayersUsed).</summary>
    [Shim(GdkModule.Name, "XGameUiShowPlayerPickerResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowPlayerPickerResult(nint async, uint count, ulong* players, uint* used)
    {
        try
        {
            return NoPlayers(async, used);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowPlayerPickerResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, TextEntryApi)]
    [UnmanagedCallersOnly]
    private static int XGameUiShowTextEntryAsync(nint async)
    {
        try
        {
            return Begin(TextEntryApi, async, DismissedUiOperation.Canceled());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowTextEntryAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowTextEntryResultSize")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowTextEntryResultSize(nint async, uint* size)
    {
        try
        {
            return Result(TextEntryApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowTextEntryResultSize");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowTextEntryResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowTextEntryResult(nint async, uint size, byte* text, uint* used)
    {
        try
        {
            return Result(TextEntryApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowTextEntryResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, WebAuthenticationApi)]
    [Shim(GdkModule.Name, "XGameUiShowWebAuthenticationWithOptionsAsync")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowWebAuthenticationAsync(nint async)
    {
        try
        {
            return Begin(WebAuthenticationApi, async, DismissedUiOperation.Canceled());
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowWebAuthenticationAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowWebAuthenticationResultSize")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowWebAuthenticationResultSize(nint async, nuint* size)
    {
        try
        {
            return Result(WebAuthenticationApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowWebAuthenticationResultSize");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XGameUiShowWebAuthenticationResult")]
    [UnmanagedCallersOnly]
    private static int XGameUiShowWebAuthenticationResult(nint async, nuint size, nint buffer, nint* result, nuint* used)
    {
        try
        {
            return Result(WebAuthenticationApi, async, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XGameUiShim.XGameUiShowWebAuthenticationResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XGameUiSetNotificationPositionHint(XGameUiNotificationPositionHint): there are no notifications to place.</summary>
    [Shim(GdkModule.Name, "XGameUiSetNotificationPositionHint")]
    [UnmanagedCallersOnly]
    private static int XGameUiSetNotificationPositionHint(uint position)
    {
        return HResult.Ok;
    }

    private static int Begin(string api, nint async, ManagedAsyncOperation operation)
    {
        var gdk = ShimContext.Runtime.Gdk;

        try
        {
            return gdk.Async.BeginManaged(async, api, operation);
        }
        catch (Exception e)
        {
            return gdk.Boundary.Failed(api, e);
        }
    }

    private static int Result(string api, nint async, nuint size, nint buffer)
    {
        var gdk = ShimContext.Runtime.Gdk;

        try
        {
            return gdk.Async.ManagedResult(async, api, size, buffer);
        }
        catch (Exception e)
        {
            return gdk.Boundary.Failed(api, e);
        }
    }

    private static int NoPlayers(nint async, uint* count)
    {
        int hr = Result(PlayerPickerApi, async, 0, 0);

        if (count != null && !HResult.Failed(hr))
        {
            *count = 0;
        }

        return hr;
    }
}
