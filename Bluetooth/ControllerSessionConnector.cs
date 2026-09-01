using Windows.Devices.Bluetooth;

namespace Ns2Pro.BleBridge;

internal sealed class ControllerSessionConnector(CliOptions options, Logger logger)
{
    private enum DeviceSource { Explicit, Cache, Scan }

    private readonly record struct DeviceResolution(ulong Address, DeviceSource Source);

    public async Task<BleController> ConnectAsync(CancellationToken ct)
    {
        var resolution = await ResolveDeviceAddressAsync(ct).ConfigureAwait(false);
        var controller = new BleController(logger, options.FeatureFlags);

        try
        {
            logger.Info($"Connecting BLE controller {BluetoothAddress.Format(resolution.Address)}.");
            await controller.ConnectAndInitializeAsync(resolution.Address, ct).ConfigureAwait(false);

            if (ShouldPairHost(resolution))
            {
                var host = options.HostAddress ?? await GetLocalBluetoothAddressAsync(ct).ConfigureAwait(false);
                logger.Info($"Pairing controller to local host {BluetoothAddress.Format(host)}.");
                await NS2ProPairing.PairHostAsync(controller, host, ct).ConfigureAwait(false);
                logger.Info("Host pairing completed.");
            }

            CachedControllerStore.Save(options.CacheFile, resolution.Address);
            return controller;
        }
        catch
        {
            await controller.DisposeAsync().ConfigureAwait(false);
            if (resolution.Source == DeviceSource.Cache && !ct.IsCancellationRequested)
            {
                CachedControllerStore.Delete(options.CacheFile);
                logger.Warn("Cached BLE controller did not connect; cache cleared so the next retry can scan for a pairing controller.");
            }

            throw;
        }
    }

    private async Task<DeviceResolution> ResolveDeviceAddressAsync(CancellationToken ct)
    {
        if (!string.IsNullOrWhiteSpace(options.DeviceAddress))
        {
            return new DeviceResolution(
                BluetoothAddress.Parse(options.DeviceAddress),
                DeviceSource.Explicit);
        }

        var cached = CachedControllerStore.Load(options.CacheFile);
        if (cached is { } cachedAddress)
        {
            logger.Info($"Using cached BLE controller {BluetoothAddress.Format(cachedAddress)}.");
            return new DeviceResolution(
                cachedAddress,
                DeviceSource.Cache);
        }

        var found = await BleController.ScanAsync(logger, ct).ConfigureAwait(false);
        logger.Info($"Found {found.Name} at {BluetoothAddress.Format(found.Address)}.");
        return new DeviceResolution(found.Address, DeviceSource.Scan);
    }

    private bool ShouldPairHost(DeviceResolution resolution) =>
        resolution.Source == DeviceSource.Scan || options.PairKnownDevice;

    private static async Task<ulong> GetLocalBluetoothAddressAsync(CancellationToken ct)
    {
        var adapter = await BluetoothAdapter.GetDefaultAsync().AsTask(ct).ConfigureAwait(false)
            ?? throw new InvalidOperationException("No default Bluetooth adapter is available.");
        if (adapter.BluetoothAddress == 0)
        {
            throw new InvalidOperationException("Default Bluetooth adapter did not report a valid address.");
        }

        return adapter.BluetoothAddress;
    }
}
