using System.Buffers.Binary;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Advertisement;
using Windows.Devices.Bluetooth.GenericAttributeProfile;
using Windows.Foundation;
using Windows.Storage.Streams;

namespace Ns2Pro.BleBridge;

internal sealed class BleController(Logger logger, byte featureFlags) : IAsyncDisposable
{
    private const int RumbleMinIntervalMs = 20;
    private const int StatsIntervalMs = 5_000;

    private static readonly Guid InitUuid = Guid.Parse("00c5af5d-1964-4e30-8f51-1956f96bd282");
    private static readonly Guid InputUuid = Guid.Parse("ab7de9be-89fe-49ad-828f-118f09df7fd2");
    private static readonly Guid VibrationUuid = Guid.Parse("cc483f51-9258-427d-a939-630c31f72b05");
    private static readonly Guid CommandUuid = Guid.Parse("649d4ac9-8eb7-4e6c-af44-1ea54fe5f005");
    private static readonly Guid CommandResponseUuid = Guid.Parse("c765a961-d9d8-4d36-a20a-5315b111836a");

    private BluetoothLEDevice? _device;
    private BluetoothLEPreferredConnectionParametersRequest? _connectionRequest;
    private readonly Dictionary<Guid, GattCharacteristic> _characteristics = [];
    private readonly SemaphoreSlim _commandSemaphore = new(1, 1);
    private readonly object _rumbleLock = new();
    private byte[]? _pendingRumble;
    private bool _rumbleWriteInProgress;
    private StickCalibration? _primaryStick;
    private StickCalibration? _secondaryStick;
    private TaskCompletionSource<byte[]> _nextCommandResponse = NewResponseSource();
    private long _inputReportCount;
    private long _rumbleRequestCount;
    private long _rumbleWriteCount;
    private long _lastStatsTicks = Environment.TickCount64;
    private long _lastAllocatedBytes = GC.GetTotalAllocatedBytes(precise: false);

    public event Action<NS2ProInputState>? InputReceived;

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
            var name = e.Advertisement.LocalName;
            if (!name.Contains("Switch 2 Pro", StringComparison.OrdinalIgnoreCase))
            {
                return;
            }
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
        _device = await BluetoothLEDevice.FromBluetoothAddressAsync(address).AsTask(ct).ConfigureAwait(false)
            ?? throw new InvalidOperationException($"Could not connect to BLE device {BluetoothAddress.Format(address)}.");

        RequestThroughputOptimized();
        await DiscoverAsync(ct).ConfigureAwait(false);

        await WriteAsync(InitUuid, [0x01, 0x00], ct).ConfigureAwait(false);
        await EnableNotificationsAsync(CommandResponseUuid, OnCommandResponse, ct).ConfigureAwait(false);
        await SetPlayerLedsAsync(NS2ProProtocol.DefaultLedPattern, ct).ConfigureAwait(false);
        await LoadStickCalibrationAsync(ct).ConfigureAwait(false);
        await SendCommandAsync(NS2ProProtocol.Command(0x0C, 0x02, [0xFF, 0, 0, 0]), ct).ConfigureAwait(false);
        await SendCommandAsync(NS2ProProtocol.Command(0x0C, 0x04, [featureFlags, 0, 0, 0]), ct).ConfigureAwait(false);
        await EnableNotificationsAsync(InputUuid, OnInputReport, ct).ConfigureAwait(false);
    }

    public async Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct)
    {
        await _commandSemaphore.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            var responseSource = NewResponseSource();
            _nextCommandResponse = responseSource;
            await WriteAsync(CommandUuid, command, ct).ConfigureAwait(false);
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(2));
            using var linked = CancellationTokenSource.CreateLinkedTokenSource(ct, timeout.Token);
            await using var _ = linked.Token.Register(static state => ((TaskCompletionSource<byte[]>)state!).TrySetCanceled(), responseSource);
            return await responseSource.Task.ConfigureAwait(false);
        }
        finally
        {
            _commandSemaphore.Release();
        }
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
        foreach (var ch in _characteristics.Values)
        {
            ch.ValueChanged -= OnCommandResponse;
            ch.ValueChanged -= OnInputReport;
        }
        if (_connectionRequest is not null)
        {
            _connectionRequest.Dispose();
            _connectionRequest = null;
        }
        _device?.Dispose();
        _commandSemaphore.Dispose();
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
                await WriteAsync(VibrationUuid, packet, cts.Token).ConfigureAwait(false);
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

    private async Task DiscoverAsync(CancellationToken ct)
    {
        var result = await _device!.GetGattServicesAsync(BluetoothCacheMode.Uncached).AsTask(ct).ConfigureAwait(false);
        if (result.Status != GattCommunicationStatus.Success)
        {
            throw new InvalidOperationException($"GATT service discovery failed: {result.Status}");
        }

        foreach (var service in result.Services)
        {
            var chars = await service.GetCharacteristicsAsync(BluetoothCacheMode.Uncached).AsTask(ct).ConfigureAwait(false);
            if (chars.Status != GattCommunicationStatus.Success)
            {
                continue;
            }
            foreach (var ch in chars.Characteristics)
            {
                _characteristics[ch.Uuid] = ch;
            }
        }

        foreach (var required in new[] { InitUuid, InputUuid, VibrationUuid, CommandUuid, CommandResponseUuid })
        {
            if (!_characteristics.ContainsKey(required))
            {
                throw new InvalidOperationException($"Required GATT characteristic missing: {required}");
            }
        }
    }

    private void RequestThroughputOptimized()
    {
        try
        {
            _connectionRequest = _device!.RequestPreferredConnectionParameters(BluetoothLEPreferredConnectionParameters.ThroughputOptimized);
            logger.Info($"Requested BLE throughput-optimized connection parameters: {_connectionRequest.Status}");
        }
        catch (Exception ex)
        {
            logger.Debug($"Could not request throughput-optimized BLE connection parameters: {ex.Message}");
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

    private async Task EnableNotificationsAsync(Guid uuid, TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs> handler, CancellationToken ct)
    {
        var ch = _characteristics[uuid];
        ch.ValueChanged += handler;
        var status = await ch.WriteClientCharacteristicConfigurationDescriptorAsync(GattClientCharacteristicConfigurationDescriptorValue.Notify).AsTask(ct).ConfigureAwait(false);
        if (status != GattCommunicationStatus.Success)
        {
            ch.ValueChanged -= handler;
            throw new InvalidOperationException($"Failed to enable notifications for {uuid}: {status}");
        }
    }

    private async Task WriteAsync(Guid uuid, byte[] data, CancellationToken ct)
    {
        using var writer = new DataWriter();
        writer.WriteBytes(data);
        var status = await _characteristics[uuid].WriteValueAsync(writer.DetachBuffer(), GattWriteOption.WriteWithoutResponse).AsTask(ct).ConfigureAwait(false);
        if (status != GattCommunicationStatus.Success)
        {
            throw new InvalidOperationException($"GATT write failed for {uuid}: {status}");
        }
    }

    private void OnCommandResponse(GattCharacteristic sender, GattValueChangedEventArgs args)
    {
        var data = ReadBytes(args.CharacteristicValue);
        _nextCommandResponse.TrySetResult(data);
    }

    private void OnInputReport(GattCharacteristic sender, GattValueChangedEventArgs args)
    {
        try
        {
            var data = ReadBytes(args.CharacteristicValue);
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

    private static byte[] ReadBytes(IBuffer buffer)
    {
        using var reader = DataReader.FromBuffer(buffer);
        var data = new byte[reader.UnconsumedBufferLength];
        reader.ReadBytes(data);
        return data;
    }

    private static TaskCompletionSource<byte[]> NewResponseSource() =>
        new(TaskCreationOptions.RunContinuationsAsynchronously);

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
