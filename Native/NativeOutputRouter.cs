using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Ns2Pro.BleBridge;

internal static class NativeOutputRouter
{
    private static readonly ConcurrentDictionary<nuint, IControllerOutputTarget> s_outputTargets = [];

    public static void SetTarget(nuint deviceHandle, IControllerOutputTarget? target)
    {
        if (deviceHandle == 0)
        {
            return;
        }

        if (target is null)
        {
            s_outputTargets.TryRemove(deviceHandle, out _);
            return;
        }

        s_outputTargets[deviceHandle] = target;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static unsafe void OnNativeOutput(nuint handle, byte* leftRumble, byte* rightRumble, byte flags, byte playerLedMask)
    {
        if (!s_outputTargets.TryGetValue(handle, out var target))
        {
            return;
        }

        if ((flags & NS2ProProtocol.OutputFlagRumble) != 0)
        {
            target.SendRumble(
                new ReadOnlySpan<byte>(leftRumble, 16),
                new ReadOnlySpan<byte>(rightRumble, 16));
        }

        if ((flags & NS2ProProtocol.OutputFlagLed) != 0)
        {
            _ = Task.Run(() => RunLedOutputAsync(target, playerLedMask));
        }
    }

    private static async Task RunLedOutputAsync(IControllerOutputTarget target, byte playerLedMask)
    {
        try
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(2));
            await target.SetPlayerLedsAsync(playerLedMask, cts.Token).ConfigureAwait(false);
        }
        catch
        {
            // Native output callbacks cannot surface async failures safely.
        }
    }
}
