using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Ns2Pro.BleBridge;

internal sealed class BridgeApp : IDisposable
{
    private static BleController? s_activeController;

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
            if (File.Exists(_options.CacheFile))
            {
                File.Delete(_options.CacheFile);
            }
            _logger.Info("Forgot cached BLE controller.");
            return 0;
        }

        _server.Start(_options.UsbAddr, autoAttach: !_options.NoAutoAttach);
        _server.Update(NS2ProInputState.Default);

        while (!_stop.IsCancellationRequested)
        {
            try
            {
                var address = await ResolveDeviceAddressAsync(_stop.Token).ConfigureAwait(false);
                _logger.Info($"Connecting BLE controller {BluetoothAddress.Format(address)}.");

                await using var controller = new BleController(_logger, _options.FeatureFlags);
                s_activeController = controller;
                controller.InputReceived += _server.Update;
                await controller.ConnectAndInitializeAsync(address, _stop.Token).ConfigureAwait(false);
                BluetoothAddress.SaveCached(_options.CacheFile, address);

                if (_options.PairHost && _options.HostAddress is { } host)
                {
                    await NS2ProProtocol.PairHostAsync(controller, host, _stop.Token).ConfigureAwait(false);
                    _logger.Info("pair-host completed.");
                }

                _logger.Info("BLE controller initialized.");
                await Task.Delay(Timeout.InfiniteTimeSpan, _stop.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (_stop.IsCancellationRequested)
            {
                break;
            }
            catch (Exception ex)
            {
                s_activeController = null;
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

        var left = Array.Empty<byte>();
        var right = Array.Empty<byte>();
        if ((flags & NS2ProProtocol.OutputFlagRumble) != 0)
        {
            left = new byte[16];
            right = new byte[16];
            new ReadOnlySpan<byte>(leftRumble, 16).CopyTo(left);
            new ReadOnlySpan<byte>(rightRumble, 16).CopyTo(right);
        }

        _ = Task.Run(() => RunOutputAsync(controller, left, right, flags, playerLedMask));
    }

    private static async Task RunOutputAsync(BleController controller, byte[] left, byte[] right, byte flags, byte playerLedMask)
    {
        try
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(2));
            if ((flags & NS2ProProtocol.OutputFlagRumble) != 0)
            {
                await controller.SendRumbleAsync(left, right, cts.Token).ConfigureAwait(false);
            }
            if ((flags & NS2ProProtocol.OutputFlagLed) != 0)
            {
                await controller.SetPlayerLedsAsync(playerLedMask, cts.Token).ConfigureAwait(false);
            }
        }
        catch
        {
            // Native output callbacks cannot surface async failures safely.
        }
    }

    private async Task<ulong> ResolveDeviceAddressAsync(CancellationToken ct)
    {
        if (!string.IsNullOrWhiteSpace(_options.DeviceAddress))
        {
            return BluetoothAddress.Parse(_options.DeviceAddress);
        }
        var cached = BluetoothAddress.LoadCached(_options.CacheFile);
        if (cached is { } cachedAddress)
        {
            _logger.Info($"Using cached BLE controller {BluetoothAddress.Format(cachedAddress)}.");
            return cachedAddress;
        }
        var found = await BleController.ScanAsync(_logger, ct).ConfigureAwait(false);
        _logger.Info($"Found {found.Name} at {BluetoothAddress.Format(found.Address)}.");
        return found.Address;
    }

    private void OnCancelKeyPress(object? sender, ConsoleCancelEventArgs e)
    {
        e.Cancel = true;
        _stop.Cancel();
    }
}
