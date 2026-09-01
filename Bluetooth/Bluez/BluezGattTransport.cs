using Ns2Pro.BleBridge.Bluez;
using Tmds.DBus.Protocol;

namespace Ns2Pro.BleBridge;

internal sealed class BluezGattTransport : IBleTransport
{
    private const string CharacteristicInterface = "org.bluez.GattCharacteristic1";
    private static readonly Guid s_initUuid = Guid.Parse("00c5af5d-1964-4e30-8f51-1956f96bd282");
    private static readonly Guid s_inputUuid = Guid.Parse("ab7de9be-89fe-49ad-828f-118f09df7fd2");
    private static readonly Guid s_vibrationUuid = Guid.Parse("cc483f51-9258-427d-a939-630c31f72b05");
    private static readonly Guid s_commandUuid = Guid.Parse("649d4ac9-8eb7-4e6c-af44-1ea54fe5f005");
    private static readonly Guid s_commandResponseUuid = Guid.Parse("c765a961-d9d8-4d36-a20a-5315b111836a");

    private readonly DBusConnection _connection;
    private readonly Device1 _device;
    private readonly Logger _logger;
    private readonly Dictionary<Guid, GattCharacteristic1> _characteristics;
    private readonly SemaphoreSlim _commandSemaphore = new(1, 1);
    private readonly List<(GattCharacteristic1 Characteristic, IDisposable Watch)> _notifications = [];
    private readonly TaskCompletionSource _disconnected = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private TaskCompletionSource<byte[]> _nextCommandResponse = NewResponseSource();
    private IDisposable? _deviceWatch;
    private bool _disposed;

    private BluezGattTransport(
        DBusConnection connection,
        Device1 device,
        Dictionary<Guid, GattCharacteristic1> characteristics,
        Logger logger)
    {
        _connection = connection;
        _device = device;
        _characteristics = characteristics;
        _logger = logger;
    }

    public Task Disconnected => _disconnected.Task;

    public static async Task<BluezGattTransport> CreateAsync(
        DBusConnection connection,
        DBusService service,
        ObjectManager manager,
        Device1 device,
        ObjectPath devicePath,
        Logger logger,
        CancellationToken ct)
    {
        var characteristics = await DiscoverCharacteristicsAsync(service, manager, devicePath, ct).ConfigureAwait(false);
        var transport = new BluezGattTransport(connection, device, characteristics, logger);
        transport._deviceWatch = await device.WatchPropertiesChangedAsync(props =>
        {
            if (props.HasConnectedChanged && props.Connected == false && transport._disconnected.TrySetResult())
            {
                logger.Warn("BlueZ reported that the BLE controller disconnected.");
            }
        }, emitOnCapturedContext: false).ConfigureAwait(false);
        _ = connection.DisconnectedAsync().ContinueWith(
            static (_, state) => ((TaskCompletionSource)state!).TrySetResult(),
            transport._disconnected,
            CancellationToken.None,
            TaskContinuationOptions.ExecuteSynchronously,
            TaskScheduler.Default);
        return transport;
    }

    public async Task InitializeAsync(CancellationToken ct)
    {
        await WriteAsync(s_initUuid, [0x01, 0x00], ct).ConfigureAwait(false);
        await EnableNotificationsAsync(s_commandResponseUuid, data => _nextCommandResponse.TrySetResult(data), ct).ConfigureAwait(false);
    }

    public Task EnableInputReportsAsync(Action<byte[]> handler, CancellationToken ct) =>
        EnableNotificationsAsync(s_inputUuid, handler, ct);

    public async Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct)
    {
        await _commandSemaphore.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            var response = NewResponseSource();
            _nextCommandResponse = response;
            await WriteAsync(s_commandUuid, command, ct).ConfigureAwait(false);
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
            timeout.CancelAfter(TimeSpan.FromSeconds(2));
            return await response.Task.WaitAsync(timeout.Token).ConfigureAwait(false);
        }
        finally
        {
            _commandSemaphore.Release();
        }
    }

    public Task WriteVibrationAsync(byte[] packet, CancellationToken ct) => WriteAsync(s_vibrationUuid, packet, ct);

    public async ValueTask DisposeAsync()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        _deviceWatch?.Dispose();
        _deviceWatch = null;
        foreach (var (characteristic, watch) in _notifications)
        {
            watch.Dispose();
            try { await characteristic.StopNotifyAsync().ConfigureAwait(false); }
            catch (Exception ex) { _logger.Debug($"Could not stop BlueZ notification: {ex.Message}"); }
        }
        _notifications.Clear();
        try { await _device.DisconnectAsync().ConfigureAwait(false); }
        catch (Exception ex) { _logger.Debug($"Could not disconnect BlueZ device: {ex.Message}"); }
        _disconnected.TrySetResult();
        _commandSemaphore.Dispose();
    }

    private async Task WriteAsync(Guid uuid, byte[] value, CancellationToken ct)
    {
        await _characteristics[uuid].WriteValueAsync(value, new Dictionary<string, VariantValue>
        {
            ["type"] = VariantValue.String("command")
        }).WaitAsync(ct).ConfigureAwait(false);
    }

    private async Task EnableNotificationsAsync(Guid uuid, Action<byte[]> handler, CancellationToken ct)
    {
        var characteristic = _characteristics[uuid];
        var watch = await characteristic.WatchPropertiesChangedAsync(props =>
        {
            if (props.HasValueChanged && props.Value is { } value)
            {
                handler(value);
            }
        }, emitOnCapturedContext: false).ConfigureAwait(false);
        try
        {
            await characteristic.StartNotifyAsync().WaitAsync(ct).ConfigureAwait(false);
            _notifications.Add((characteristic, watch));
        }
        catch
        {
            watch.Dispose();
            throw;
        }
    }

    private static async Task<Dictionary<Guid, GattCharacteristic1>> DiscoverCharacteristicsAsync(
        DBusService service,
        ObjectManager manager,
        ObjectPath devicePath,
        CancellationToken ct)
    {
        var required = new HashSet<Guid> { s_initUuid, s_inputUuid, s_vibrationUuid, s_commandUuid, s_commandResponseUuid };
        var found = new Dictionary<Guid, GattCharacteristic1>();
        var prefix = devicePath + "/";
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromSeconds(15));

        while (found.Count < required.Count)
        {
            var objects = await manager.GetManagedObjectsAsync().WaitAsync(timeout.Token).ConfigureAwait(false);
            foreach (var (path, interfaces) in objects)
            {
                if (!path.ToString().StartsWith(prefix, StringComparison.Ordinal) ||
                    !interfaces.ContainsKey(CharacteristicInterface))
                {
                    continue;
                }
                var characteristic = service.CreateGattCharacteristic1(path);
                if (Guid.TryParse(await characteristic.GetUUIDAsync().WaitAsync(timeout.Token).ConfigureAwait(false), out var uuid) &&
                    required.Contains(uuid))
                {
                    found[uuid] = characteristic;
                }
            }
            if (found.Count < required.Count)
            {
                await Task.Delay(TimeSpan.FromMilliseconds(200), timeout.Token).ConfigureAwait(false);
            }
        }
        return found;
    }

    private static TaskCompletionSource<byte[]> NewResponseSource() =>
        new(TaskCreationOptions.RunContinuationsAsynchronously);
}
