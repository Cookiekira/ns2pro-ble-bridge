namespace Ns2Pro.BleBridge;

internal static class NativeOutputRunner
{
    public static async Task RunLedOutputAsync(IControllerOutputTarget target, byte playerLedMask)
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
