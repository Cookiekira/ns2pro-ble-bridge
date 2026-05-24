using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.GenericAttributeProfile;
using Windows.Foundation;
using Windows.Storage.Streams;

namespace Ns2Pro.BleBridge;

internal sealed class BleGattTransport(BluetoothLEDevice device, Logger logger, Action<string> disconnected) : IControllerCommandChannel, IAsyncDisposable
{
    public static readonly Guid InitUuid = Guid.Parse("00c5af5d-1964-4e30-8f51-1956f96bd282");
    public static readonly Guid InputUuid = Guid.Parse("ab7de9be-89fe-49ad-828f-118f09df7fd2");
    public static readonly Guid VibrationUuid = Guid.Parse("cc483f51-9258-427d-a939-630c31f72b05");
    public static readonly Guid CommandUuid = Guid.Parse("649d4ac9-8eb7-4e6c-af44-1ea54fe5f005");
    public static readonly Guid CommandResponseUuid = Guid.Parse("c765a961-d9d8-4d36-a20a-5315b111836a");

    private readonly Dictionary<Guid, GattCharacteristic> _characteristics = [];
    private readonly SemaphoreSlim _commandSemaphore = new(1, 1);
    private readonly List<(Guid Uuid, TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs> Handler)> _handlers = [];
    private BluetoothLEPreferredConnectionParametersRequest? _connectionRequest;
    private TaskCompletionSource<byte[]> _nextCommandResponse = NewResponseSource();

    public void RequestThroughputOptimized()
    {
        try
        {
            _connectionRequest = device.RequestPreferredConnectionParameters(BluetoothLEPreferredConnectionParameters.ThroughputOptimized);
            logger.Info($"Requested BLE throughput-optimized connection parameters: {_connectionRequest.Status}");
        }
        catch (Exception ex)
        {
            logger.Debug($"Could not request throughput-optimized BLE connection parameters: {ex.Message}");
        }
    }

    public async Task DiscoverAsync(CancellationToken ct)
    {
        var result = await device.GetGattServicesAsync(BluetoothCacheMode.Uncached).AsTask(ct).ConfigureAwait(false);
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

    public Task EnableCommandResponsesAsync(CancellationToken ct) =>
        EnableNotificationsAsync(CommandResponseUuid, OnCommandResponse, ct);

    public Task EnableInputReportsAsync(
        TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs> handler,
        CancellationToken ct) =>
        EnableNotificationsAsync(InputUuid, handler, ct);

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
        catch (Exception ex) when (ex is not OperationCanceledException || !ct.IsCancellationRequested)
        {
            disconnected($"BLE command failed: {ex.Message}");
            throw;
        }
        finally
        {
            _commandSemaphore.Release();
        }
    }

    public Task WriteVibrationAsync(byte[] packet, CancellationToken ct) =>
        WriteAsync(VibrationUuid, packet, ct);

    public async Task WriteAsync(Guid uuid, byte[] data, CancellationToken ct)
    {
        using var writer = new DataWriter();
        writer.WriteBytes(data);
        try
        {
            var status = await _characteristics[uuid].WriteValueAsync(writer.DetachBuffer(), GattWriteOption.WriteWithoutResponse).AsTask(ct).ConfigureAwait(false);
            if (status != GattCommunicationStatus.Success)
            {
                throw new InvalidOperationException($"GATT write failed for {uuid}: {status}");
            }
        }
        catch (Exception ex) when (ex is not OperationCanceledException || !ct.IsCancellationRequested)
        {
            disconnected($"GATT write failed for {uuid}: {ex.Message}");
            throw;
        }
    }

    public ValueTask DisposeAsync()
    {
        foreach (var (uuid, handler) in _handlers)
        {
            if (_characteristics.TryGetValue(uuid, out var ch))
            {
                ch.ValueChanged -= handler;
            }
        }

        _handlers.Clear();
        _connectionRequest?.Dispose();
        _connectionRequest = null;
        _commandSemaphore.Dispose();
        return ValueTask.CompletedTask;
    }

    private async Task EnableNotificationsAsync(Guid uuid, TypedEventHandler<GattCharacteristic, GattValueChangedEventArgs> handler, CancellationToken ct)
    {
        var ch = _characteristics[uuid];
        ch.ValueChanged += handler;
        try
        {
            var status = await ch.WriteClientCharacteristicConfigurationDescriptorAsync(GattClientCharacteristicConfigurationDescriptorValue.Notify).AsTask(ct).ConfigureAwait(false);
            if (status != GattCommunicationStatus.Success)
            {
                throw new InvalidOperationException($"Failed to enable notifications for {uuid}: {status}");
            }

            _handlers.Add((uuid, handler));
        }
        catch (Exception ex) when (ex is not OperationCanceledException || !ct.IsCancellationRequested)
        {
            ch.ValueChanged -= handler;
            disconnected($"GATT notification setup failed for {uuid}: {ex.Message}");
            throw;
        }
        catch
        {
            ch.ValueChanged -= handler;
            throw;
        }
    }

    private void OnCommandResponse(GattCharacteristic sender, GattValueChangedEventArgs args)
    {
        var data = BufferReader.ReadBytes(args.CharacteristicValue);
        _nextCommandResponse.TrySetResult(data);
    }

    private static TaskCompletionSource<byte[]> NewResponseSource() =>
        new(TaskCreationOptions.RunContinuationsAsynchronously);
}
