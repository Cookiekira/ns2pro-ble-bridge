#if WINDOWS
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Advertisement;

namespace Ns2Pro.BleBridge;

internal sealed class WindowsBluetoothBackend(Logger logger) : IBluetoothBackend
{
    public bool SupportsHostPairing => true;

    public async Task<BleDeviceInfo> ScanAsync(CancellationToken ct)
    {
        var watcher = new BluetoothLEAdvertisementWatcher
        {
            ScanningMode = BluetoothLEScanningMode.Active,
            AllowExtendedAdvertisements = true
        };
        var found = new TaskCompletionSource<BleDeviceInfo>(TaskCreationOptions.RunContinuationsAsynchronously);

        watcher.Received += (_, e) =>
        {
            if (!Switch2ProAdvertisement.TryParse(e.Advertisement, out var advertisement))
            {
                return;
            }

            var name = string.IsNullOrWhiteSpace(e.Advertisement.LocalName)
                ? $"Switch 2 Pro Controller ({advertisement.ModeName})"
                : e.Advertisement.LocalName;
            found.TrySetResult(new BleDeviceInfo(e.BluetoothAddress, name));
        };

        logger.Info("Scanning for Switch 2 Pro Controller over BLE.");
        try
        {
            watcher.Start();
            await using var _ = ct.Register(static state => ((TaskCompletionSource<BleDeviceInfo>)state!).TrySetCanceled(), found);
            return await found.Task.ConfigureAwait(false);
        }
        finally
        {
            watcher.Stop();
        }
    }

    public async Task<IBleTransport> ConnectAsync(ulong address, CancellationToken ct)
    {
        var device = await BluetoothLEDevice.FromBluetoothAddressAsync(address).AsTask(ct).ConfigureAwait(false)
            ?? throw new InvalidOperationException($"Could not connect to BLE device {BluetoothAddress.Format(address)}.");
        return new WindowsGattTransport(device, logger);
    }

    public static async Task<ulong> GetDefaultAdapterAddressAsync(CancellationToken ct)
    {
        var adapter = await BluetoothAdapter.GetDefaultAsync().AsTask(ct).ConfigureAwait(false)
            ?? throw new InvalidOperationException("No default Bluetooth adapter is available.");
        if (adapter.BluetoothAddress == 0)
        {
            throw new InvalidOperationException("Default Bluetooth adapter did not report a valid address.");
        }
        return adapter.BluetoothAddress;
    }

    public Task<ulong> GetAdapterAddressAsync(CancellationToken ct) => GetDefaultAdapterAddressAsync(ct);

    public ValueTask DisposeAsync() => ValueTask.CompletedTask;
}
#endif
