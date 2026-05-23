using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Windows.Devices.Bluetooth;

namespace Ns2Pro.BleBridge;

internal sealed class BridgeApp : IDisposable
{
    private static BleController? s_activeController;
    private enum DeviceSource { Explicit, Cache, Scan }
    private readonly record struct DeviceResolution(ulong Address, DeviceSource Source);

    private readonly CliOptions _options;
    private readonly Logger _logger;
    private readonly CancellationTokenSource _stop = new();
    private readonly ViiperServer _server;

    public BridgeApp(CliOptions options)
    {
        _options = options;
        _logger = new Logger(options.LogLevel);
        _server = new ViiperServer(_logger);
        Console.CancelKeyPress += OnCancelKeyPress;
    }

    public async Task<int> RunAsync()
    {
        if (_options.ForgetDevice)
        {
            DeleteCacheFile(_options.CacheFile);
            _logger.Info("Forgot cached BLE controller.");
            return 0;
        }

        _server.Start(_options.UsbAddr, autoAttach: !_options.NoAutoAttach);
        _server.Update(NS2ProInputState.Default);

        while (!_stop.IsCancellationRequested)
        {
            DeviceResolution? resolution = null;
            try
            {
                resolution = await ResolveDeviceAddressAsync(_stop.Token).ConfigureAwait(false);
                var address = resolution.Value.Address;
                _logger.Info($"Connecting BLE controller {BluetoothAddress.Format(address)}.");

                await using var controller = new BleController(_logger, _options.FeatureFlags);
                await controller.ConnectAndInitializeAsync(address, _stop.Token).ConfigureAwait(false);

                if (ShouldPairHost(resolution.Value))
                {
                    var host = _options.HostAddress ?? await GetLocalBluetoothAddressAsync(_stop.Token).ConfigureAwait(false);
                    _logger.Info($"Pairing controller to local host {BluetoothAddress.Format(host)}.");
                    await NS2ProProtocol.PairHostAsync(controller, host, _stop.Token).ConfigureAwait(false);
                    _logger.Info("Host pairing completed.");
                }

                BluetoothAddress.SaveCached(_options.CacheFile, address);
                controller.InputReceived += _server.Update;
                s_activeController = controller;
                _logger.Info("BLE controller initialized.");
                await Task.Delay(Timeout.InfiniteTimeSpan, _stop.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (_stop.IsCancellationRequested)
            {
                s_activeController = null;
                break;
            }
            catch (Exception ex)
            {
                s_activeController = null;
                if (resolution is { Source: DeviceSource.Cache })
                {
                    DeleteCacheFile(_options.CacheFile);
                    _logger.Warn("Cached BLE controller did not connect; cache cleared so the next retry can scan for a pairing controller.");
                }
                _logger.Error(ex, "BLE bridge failed; retrying in 3s");
                try
                {
                    await Task.Delay(TimeSpan.FromSeconds(3), _stop.Token).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    break;
                }
            }
        }

        return 0;
    }

    public void Dispose()
    {
        Console.CancelKeyPress -= OnCancelKeyPress;
        s_activeController = null;
        _stop.Cancel();
        _stop.Dispose();
        _server.Dispose();
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static unsafe void OnNativeOutput(nuint handle, byte* leftRumble, byte* rightRumble, byte flags, byte playerLedMask)
    {
        var controller = s_activeController;
        if (controller is null)
        {
            return;
        }

        if ((flags & NS2ProProtocol.OutputFlagRumble) != 0)
        {
            controller.SendRumble(
                new ReadOnlySpan<byte>(leftRumble, 16),
                new ReadOnlySpan<byte>(rightRumble, 16));
        }

        if ((flags & NS2ProProtocol.OutputFlagLed) != 0)
        {
            _ = Task.Run(() => RunLedOutputAsync(controller, playerLedMask));
        }
    }

    private static async Task RunLedOutputAsync(BleController controller, byte playerLedMask)
    {
        try
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(2));
            await controller.SetPlayerLedsAsync(playerLedMask, cts.Token).ConfigureAwait(false);
        }
        catch
        {
            // Native output callbacks cannot surface async failures safely.
        }
    }

    private async Task<DeviceResolution> ResolveDeviceAddressAsync(CancellationToken ct)
    {
        if (!string.IsNullOrWhiteSpace(_options.DeviceAddress))
        {
            return new DeviceResolution(
                BluetoothAddress.Parse(_options.DeviceAddress),
                DeviceSource.Explicit);
        }
        var cached = BluetoothAddress.LoadCached(_options.CacheFile);
        if (cached is { } cachedAddress)
        {
            _logger.Info($"Using cached BLE controller {BluetoothAddress.Format(cachedAddress)}.");
            return new DeviceResolution(
                cachedAddress,
                DeviceSource.Cache);
        }

        var found = await BleController.ScanAsync(_logger, ct).ConfigureAwait(false);
        _logger.Info($"Found {found.Name} at {BluetoothAddress.Format(found.Address)}.");
        return new DeviceResolution(found.Address, DeviceSource.Scan);
    }

    private bool ShouldPairHost(DeviceResolution resolution)
    {
        return resolution.Source == DeviceSource.Scan || _options.PairKnownDevice;
    }

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

    private void OnCancelKeyPress(object? sender, ConsoleCancelEventArgs e)
    {
        e.Cancel = true;
        _stop.Cancel();
    }

    private static void DeleteCacheFile(string path)
    {
        if (File.Exists(path))
        {
            File.Delete(path);
        }
    }
}
