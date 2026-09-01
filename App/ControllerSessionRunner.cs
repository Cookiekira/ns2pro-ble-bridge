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
        await using var controller = await connector.ConnectAsync(ct).ConfigureAwait(false);
        try
        {
            controller.InputReceived += server.Update;
            server.SetOutputTarget(controller);
            logger.Info("BLE controller initialized.");
            await controller.Disconnected.WaitAsync(ct).ConfigureAwait(false);
            logger.Warn($"BLE controller {BluetoothAddress.Format(controller.Address)} disconnected; reconnecting.");
        }
        finally
        {
            server.SetOutputTarget(null);
            controller.InputReceived -= server.Update;
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
