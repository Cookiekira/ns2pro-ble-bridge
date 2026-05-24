namespace Ns2Pro.BleBridge;

internal sealed class ControllerSessionRunner(
    ControllerSessionConnector connector,
    ViiperServer server,
    Logger logger)
{
    public async Task RunAsync(CancellationToken ct)
    {
        while (!ct.IsCancellationRequested)
        {
            try
            {
                await RunSessionAsync(ct).ConfigureAwait(false);
                await DelayAsync(TimeSpan.FromSeconds(1), ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                logger.Error(ex, "BLE bridge failed; retrying in 3s");
                await DelayAsync(TimeSpan.FromSeconds(3), ct).ConfigureAwait(false);
            }
        }
    }

    private async Task RunSessionAsync(CancellationToken ct)
    {
        await using var session = await connector.ConnectAsync(ct).ConfigureAwait(false);
        try
        {
            session.Controller.InputReceived += server.Update;
            server.SetOutputTarget(session.Controller);
            logger.Info("BLE controller initialized.");
            await session.WaitForDisconnectAsync(ct).ConfigureAwait(false);
            logger.Warn($"BLE controller {BluetoothAddress.Format(session.Address)} disconnected; reconnecting.");
        }
        finally
        {
            server.SetOutputTarget(null);
            session.Controller.InputReceived -= server.Update;
        }
    }

    private static async Task DelayAsync(TimeSpan delay, CancellationToken ct)
    {
        try
        {
            await Task.Delay(delay, ct).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
        }
    }
}
