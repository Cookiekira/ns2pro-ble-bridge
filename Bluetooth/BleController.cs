using System.Buffers.Binary;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Advertisement;

namespace Ns2Pro.BleBridge;

internal sealed class BleController(Logger logger, byte featureFlags) : IControllerCommandChannel, IControllerOutputTarget, IAsyncDisposable
{
    private const int RumbleMinIntervalMs = 20;
    private const int StatsIntervalMs = 5_000;

    private readonly TaskCompletionSource _disconnected = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private BluetoothLEDevice? _device;
    private BleGattTransport? _transport;
    private readonly object _rumbleLock = new();
    private byte[]? _pendingRumble;
    private bool _rumbleWriteInProgress;
    private StickCalibration? _primaryStick;
    private StickCalibration? _secondaryStick;
    private long _inputReportCount;
    private long _rumbleRequestCount;
    private long _rumbleWriteCount;
    private long _lastStatsTicks = Environment.TickCount64;
    private long _lastAllocatedBytes = GC.GetTotalAllocatedBytes(precise: false);

    public event Action<NS2ProInputState>? InputReceived;

    public ulong Address { get; private set; }

    public Task Disconnected => _disconnected.Task;

    public static async Task<(ulong Address, string Name)> ScanAsync(Logger logger, CancellationToken ct)
    {
        var watcher = new BluetoothLEAdvertisementWatcher
        {
            ScanningMode = BluetoothLEScanningMode.Active,
            AllowExtendedAdvertisements = true
        };
        var found = new TaskCompletionSource<(ulong Address, string Name)>(TaskCreationOptions.RunContinuationsAsynchronously);

        watcher.Received += (_, e) =>
        {
            if (!Switch2ProAdvertisement.TryParse(e.Advertisement, out var advertisement))
            {
                return;
            }

            var name = string.IsNullOrWhiteSpace(e.Advertisement.LocalName)
                ? $"Switch 2 Pro Controller ({advertisement.ModeName})"
                : e.Advertisement.LocalName;
            found.TrySetResult((e.BluetoothAddress, name));
        };

        logger.Info("Scanning for Switch 2 Pro Controller over BLE.");
        try
        {
            watcher.Start();
            await using var _ = ct.Register(static state => ((TaskCompletionSource<(ulong, string)>)state!).TrySetCanceled(), found);
            return await found.Task.ConfigureAwait(false);
        }
        finally
        {
            watcher.Stop();
        }
    }

    public async Task ConnectAndInitializeAsync(ulong address, CancellationToken ct)
    {
        Address = address;
        _device = await BluetoothLEDevice.FromBluetoothAddressAsync(address).AsTask(ct).ConfigureAwait(false)
            ?? throw new InvalidOperationException($"Could not connect to BLE device {BluetoothAddress.Format(address)}.");

        _transport = new BleGattTransport(_device, logger);
        await _transport.InitializeAsync(ct).ConfigureAwait(false);

        await SetPlayerLedsAsync(NS2ProProtocol.DefaultLedPattern, ct).ConfigureAwait(false);
        await LoadStickCalibrationAsync(ct).ConfigureAwait(false);
        await SendCommandAsync(NS2ProProtocol.Command(0x0C, 0x02, [0xFF, 0, 0, 0]), ct).ConfigureAwait(false);
        await SendCommandAsync(NS2ProProtocol.Command(0x0C, 0x04, [featureFlags, 0, 0, 0]), ct).ConfigureAwait(false);
        await _transport.EnableInputReportsAsync(OnInputReport, ct).ConfigureAwait(false);
        _device.ConnectionStatusChanged += OnConnectionStatusChanged;
        if (_device.ConnectionStatus == BluetoothConnectionStatus.Disconnected)
        {
            MarkDisconnected("BLE device reported disconnected after initialization.");
        }
    }

    public Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct)
    {
        if (_transport is not { } transport)
        {
            throw new InvalidOperationException("BLE controller is not initialized.");
        }

        return transport.SendCommandAsync(command, ct);
    }

    public Task SetPlayerLedsAsync(byte mask, CancellationToken ct) =>
        SendCommandAsync(NS2ProProtocol.BuildLedCommand(mask), ct);

    public void SendRumble(ReadOnlySpan<byte> left, ReadOnlySpan<byte> right)
    {
        var packet = NS2ProProtocol.BuildRumblePacket(left, right);
        Interlocked.Increment(ref _rumbleRequestCount);
        lock (_rumbleLock)
        {
            _pendingRumble = packet;
            if (_rumbleWriteInProgress)
            {
                return;
            }
            _rumbleWriteInProgress = true;
        }

        _ = Task.Run(ProcessRumbleLoopAsync);
    }

    public async ValueTask DisposeAsync()
    {
        if (_transport is not null)
        {
            await _transport.DisposeAsync().ConfigureAwait(false);
            _transport = null;
        }
        if (_device is not null)
        {
            _device.ConnectionStatusChanged -= OnConnectionStatusChanged;
        }
        _disconnected.TrySetResult();
        _device?.Dispose();
        _device = null;
    }

    private async Task ProcessRumbleLoopAsync()
    {
        long nextAllowedWriteTicks = 0;
        while (true)
        {
            var now = Environment.TickCount64;
            if (now < nextAllowedWriteTicks)
            {
                await Task.Delay(TimeSpan.FromMilliseconds(nextAllowedWriteTicks - now)).ConfigureAwait(false);
            }

            byte[]? packet;
            lock (_rumbleLock)
            {
                packet = _pendingRumble;
                _pendingRumble = null;
                if (packet is null)
                {
                    _rumbleWriteInProgress = false;
                    break;
                }
            }

            try
            {
                using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(1));
                if (_transport is null)
                {
                    break;
                }

                await _transport.WriteVibrationAsync(packet, cts.Token).ConfigureAwait(false);
                Interlocked.Increment(ref _rumbleWriteCount);
                nextAllowedWriteTicks = Environment.TickCount64 + RumbleMinIntervalMs;
                ReportStatsIfDue();
            }
            catch (Exception ex)
            {
                logger.Debug($"Rumble write failed: {ex.Message}");
            }
        }
    }

    private async Task LoadStickCalibrationAsync(CancellationToken ct)
    {
        try
        {
            var primaryBlock = await ReadSpiAsync(0x13080, 0x40, ct).ConfigureAwait(false);
            var secondaryBlock = await ReadSpiAsync(0x130C0, 0x40, ct).ConfigureAwait(false);
            var primary = NS2ProProtocol.UnpackStickCalibration(primaryBlock.AsSpan(0x28, 9));
            var secondary = NS2ProProtocol.UnpackStickCalibration(secondaryBlock.AsSpan(0x28, 9));

            try
            {
                var userBlock = await ReadSpiAsync(0x1FC040, 0x40, ct).ConfigureAwait(false);
                if (userBlock.Length >= 0x2B)
                {
                    if (userBlock[0] == 0xA2 && userBlock[1] == 0xB2)
                    {
                        primary = NS2ProProtocol.UnpackStickCalibration(userBlock.AsSpan(0x02, 9));
                    }
                    if (userBlock[0x20] == 0xA2 && userBlock[0x21] == 0xB2)
                    {
                        secondary = NS2ProProtocol.UnpackStickCalibration(userBlock.AsSpan(0x22, 9));
                    }
                }
            }
            catch (Exception ex)
            {
                logger.Debug($"User stick calibration read failed: {ex.Message}");
            }

            _primaryStick = primary;
            _secondaryStick = secondary;
            logger.Info("Loaded stick calibration.");
        }
        catch (Exception ex)
        {
            logger.Warn($"Could not load stick calibration; using raw stick values: {ex.Message}");
        }
    }

    private async Task<byte[]> ReadSpiAsync(uint address, byte size, CancellationToken ct)
    {
        var response = await SendCommandAsync(NS2ProProtocol.BuildSpiRead(address, size), ct).ConfigureAwait(false);
        var payload = NS2ProProtocol.ResponsePayload(response, 0x02, 0x04);
        if (payload.Length < 8)
        {
            throw new InvalidDataException("Short SPI response.");
        }
        var gotAddress = BinaryPrimitives.ReadUInt32LittleEndian(payload[4..8]);
        if (gotAddress != address)
        {
            throw new InvalidDataException($"SPI address mismatch: got 0x{gotAddress:X}, want 0x{address:X}");
        }
        var n = payload[0];
        if (payload.Length < 8 + n)
        {
            throw new InvalidDataException("Short SPI payload.");
        }
        return payload[8..(8 + n)].ToArray();
    }

    private void OnInputReport(byte[] data)
    {
        try
        {
            Interlocked.Increment(ref _inputReportCount);
            ReportStatsIfDue();
            var state = NS2ProProtocol.ParseCommonReport(data, _primaryStick, _secondaryStick);
            InputReceived?.Invoke(state);
        }
        catch (Exception ex)
        {
            logger.Debug($"Failed to parse BLE input report: {ex.Message}");
        }
    }

    private void OnConnectionStatusChanged(BluetoothLEDevice sender, object args)
    {
        try
        {
            var status = sender?.ConnectionStatus ?? _device?.ConnectionStatus;
            if (status == BluetoothConnectionStatus.Disconnected)
            {
                MarkDisconnected("BLE device connection status changed to disconnected.");
            }
        }
        catch (Exception ex)
        {
            MarkDisconnected($"BLE connection status check failed: {ex.Message}");
        }
    }

    private void MarkDisconnected(string reason)
    {
        if (_disconnected.TrySetResult())
        {
            logger.Warn(reason);
        }
    }

    private void ReportStatsIfDue()
    {
        if (!logger.IsEnabled(LogLevel.Debug))
        {
            return;
        }

        var now = Environment.TickCount64;
        var last = Interlocked.Read(ref _lastStatsTicks);
        if (now - last < StatsIntervalMs || Interlocked.CompareExchange(ref _lastStatsTicks, now, last) != last)
        {
            return;
        }

        var elapsedSeconds = Math.Max((now - last) / 1000.0, 0.001);
        var inputReports = Interlocked.Exchange(ref _inputReportCount, 0);
        var rumbleRequests = Interlocked.Exchange(ref _rumbleRequestCount, 0);
        var rumbleWrites = Interlocked.Exchange(ref _rumbleWriteCount, 0);
        var allocatedBytes = GC.GetTotalAllocatedBytes(precise: false);
        var previousAllocatedBytes = Interlocked.Exchange(ref _lastAllocatedBytes, allocatedBytes);
        var allocatedPerSecond = (allocatedBytes - previousAllocatedBytes) / elapsedSeconds;

        logger.Debug(
            $"BLE stats: input={inputReports / elapsedSeconds:F1}Hz, " +
            $"rumble-request={rumbleRequests / elapsedSeconds:F1}Hz, " +
            $"rumble-write={rumbleWrites / elapsedSeconds:F1}Hz, " +
            $"alloc={allocatedPerSecond / 1024.0:F1}KiB/s");
    }
}
