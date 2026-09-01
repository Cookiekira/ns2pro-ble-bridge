using Ns2Pro.BleBridge.Bluez;
using Tmds.DBus.Protocol;

namespace Ns2Pro.BleBridge;

internal sealed class BluezBluetoothBackend : IBluetoothBackend
{
    private const string BluezService = "org.bluez";
    private const string AdapterInterface = "org.bluez.Adapter1";
    private const string DeviceInterface = "org.bluez.Device1";

    private readonly Logger _logger;
    private readonly DBusConnection _connection;
    private readonly SemaphoreSlim _connectGate = new(1, 1);
    private DBusService? _service;
    private ObjectManager? _manager;
    private ObjectPath? _adapterPath;
    private bool _disposed;

    public BluezBluetoothBackend(Logger logger)
    {
        _logger = logger;
        var address = DBusAddress.System
            ?? throw new PlatformNotSupportedException("The D-Bus system bus address is not available.");
        _connection = new DBusConnection(new DBusConnectionOptions(address)
        {
            OnException = context => _logger.Debug($"BlueZ D-Bus {context.Source}: {context.Exception.Message}")
        });
    }

    public async Task<BleDeviceInfo> ScanAsync(CancellationToken ct)
    {
        var (adapter, _) = await GetAdapterAsync(ct).ConfigureAwait(false);
        await adapter.SetDiscoveryFilterAsync(new Dictionary<string, VariantValue>
        {
            ["Transport"] = VariantValue.String("le"),
            ["DuplicateData"] = VariantValue.Bool(true)
        }).WaitAsync(ct).ConfigureAwait(false);

        _logger.Info("Scanning for Switch 2 Pro Controller through BlueZ.");
        await adapter.StartDiscoveryAsync().WaitAsync(ct).ConfigureAwait(false);
        try
        {
            while (true)
            {
                ct.ThrowIfCancellationRequested();
                var objects = await Manager.GetManagedObjectsAsync().WaitAsync(ct).ConfigureAwait(false);
                foreach (var (path, interfaces) in objects)
                {
                    if (!interfaces.ContainsKey(DeviceInterface))
                    {
                        continue;
                    }

                    var device = Service.CreateDevice1(path);
                    var properties = await device.GetNullablePropertiesAsync().WaitAsync(ct).ConfigureAwait(false);
                    if (properties.ManufacturerData is not { } manufacturerData || !TryMatch(manufacturerData, out var match))
                    {
                        continue;
                    }

                    var address = BluetoothAddress.Parse(await device.GetAddressAsync().WaitAsync(ct).ConfigureAwait(false));
                    var name = await GetDeviceNameAsync(device, match).ConfigureAwait(false);
                    return new BleDeviceInfo(address, name);
                }

                await Task.Delay(TimeSpan.FromMilliseconds(250), ct).ConfigureAwait(false);
            }
        }
        finally
        {
            try
            {
                await adapter.StopDiscoveryAsync().ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                _logger.Debug($"Could not stop BlueZ discovery: {ex.Message}");
            }
        }
    }

    public async Task<IBleTransport> ConnectAsync(ulong address, CancellationToken ct)
    {
        var (adapter, adapterPath) = await GetAdapterAsync(ct).ConfigureAwait(false);
        var path = await FindDevicePathAsync(address, ct).ConfigureAwait(false);
        var device = Service.CreateDevice1(path);

        // Do not call Device1.Connect here.  BlueZ's GATT proxy attempts a
        // normal service discovery (and may start SMP), which causes Switch 2
        // controllers to drop the link before any characteristics are exposed.
        // The raw ATT transport owns the LE connection and keeps security low.
        try
        {
            await adapter.StopDiscoveryAsync().WaitAsync(ct).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.Debug($"Could not stop BlueZ discovery before raw ATT connect: {ex.Message}");
        }

        if (await device.GetConnectedAsync().WaitAsync(ct).ConfigureAwait(false))
        {
            try
            {
                await device.DisconnectAsync().WaitAsync(ct).ConfigureAwait(false);
                await Task.Delay(TimeSpan.FromMilliseconds(150), ct).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                _logger.Debug($"Could not clear the existing BlueZ connection: {ex.Message}");
            }
        }

        var adapterAddress = BluetoothAddress.Parse(
            await Service.CreateAdapter1(adapterPath).GetAddressAsync().WaitAsync(ct).ConfigureAwait(false));
        return await BluezRawGattTransport.CreateAsync(adapterAddress, address, _logger, ct).ConfigureAwait(false);
    }

    public async Task<ulong> GetAdapterAddressAsync(CancellationToken ct)
    {
        var (adapter, _) = await GetAdapterAsync(ct).ConfigureAwait(false);
        return BluetoothAddress.Parse(await adapter.GetAddressAsync().WaitAsync(ct).ConfigureAwait(false));
    }

    internal async Task SmokeTestAsync(CancellationToken ct)
    {
        var (adapter, _) = await GetAdapterAsync(ct).ConfigureAwait(false);
        await adapter.SetDiscoveryFilterAsync(new Dictionary<string, VariantValue>
        {
            ["Transport"] = VariantValue.String("le")
        }).WaitAsync(ct).ConfigureAwait(false);
        await adapter.StartDiscoveryAsync().WaitAsync(ct).ConfigureAwait(false);
        try
        {
            await Task.Delay(TimeSpan.FromMilliseconds(500), ct).ConfigureAwait(false);
        }
        finally
        {
            await adapter.StopDiscoveryAsync().ConfigureAwait(false);
        }
        _logger.Info("BlueZ system-bus adapter/discovery smoke test passed.");
    }

    public ValueTask DisposeAsync()
    {
        if (!_disposed)
        {
            _disposed = true;
            _connectGate.Dispose();
            _connection.Dispose();
        }
        return ValueTask.CompletedTask;
    }

    private DBusService Service => _service ?? throw new InvalidOperationException("BlueZ is not connected.");
    private ObjectManager Manager => _manager ?? throw new InvalidOperationException("BlueZ is not connected.");

    private async Task<(Adapter1 Adapter, ObjectPath Path)> GetAdapterAsync(CancellationToken ct)
    {
        await EnsureConnectedAsync(ct).ConfigureAwait(false);
        if (_adapterPath is { } selected)
        {
            return (Service.CreateAdapter1(selected), selected);
        }

        var objects = await Manager.GetManagedObjectsAsync().WaitAsync(ct).ConfigureAwait(false);
        foreach (var (path, interfaces) in objects)
        {
            if (!interfaces.ContainsKey(AdapterInterface))
            {
                continue;
            }

            var adapter = Service.CreateAdapter1(path);
            if (!await adapter.GetPoweredAsync().WaitAsync(ct).ConfigureAwait(false))
            {
                await adapter.SetPoweredAsync(true).WaitAsync(ct).ConfigureAwait(false);
            }
            _adapterPath = path;
            _logger.Info($"Using BlueZ adapter {await adapter.GetAddressAsync().WaitAsync(ct).ConfigureAwait(false)} ({path}).");
            return (adapter, path);
        }

        throw new InvalidOperationException("BlueZ did not report a usable Bluetooth adapter.");
    }

    private async Task EnsureConnectedAsync(CancellationToken ct)
    {
        if (_manager is not null)
        {
            return;
        }

        await _connectGate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            if (_manager is not null)
            {
                return;
            }
            await _connection.ConnectAsync().AsTask().WaitAsync(ct).ConfigureAwait(false);
            _service = new DBusService(_connection, BluezService);
            _manager = Service.CreateObjectManager("/");
        }
        finally
        {
            _connectGate.Release();
        }
    }

    private async Task<ObjectPath> FindDevicePathAsync(ulong address, CancellationToken ct)
    {
        var objects = await Manager.GetManagedObjectsAsync().WaitAsync(ct).ConfigureAwait(false);
        if (await FindInObjectsAsync(objects, address, ct).ConfigureAwait(false) is { } existing)
        {
            return existing;
        }

        var (adapter, _) = await GetAdapterAsync(ct).ConfigureAwait(false);
        await adapter.StartDiscoveryAsync().WaitAsync(ct).ConfigureAwait(false);
        try
        {
            while (true)
            {
                ct.ThrowIfCancellationRequested();
                objects = await Manager.GetManagedObjectsAsync().WaitAsync(ct).ConfigureAwait(false);
                if (await FindInObjectsAsync(objects, address, ct).ConfigureAwait(false) is { } discovered)
                {
                    return discovered;
                }
                await Task.Delay(TimeSpan.FromMilliseconds(250), ct).ConfigureAwait(false);
            }
        }
        finally
        {
            try { await adapter.StopDiscoveryAsync().ConfigureAwait(false); }
            catch (Exception ex) { _logger.Debug($"Could not stop BlueZ discovery: {ex.Message}"); }
        }
    }

    private async Task<ObjectPath?> FindInObjectsAsync(
        Dictionary<ObjectPath, Dictionary<string, Dictionary<string, VariantValue>>> objects,
        ulong address,
        CancellationToken ct)
    {
        foreach (var (path, interfaces) in objects)
        {
            if (!interfaces.ContainsKey(DeviceInterface))
            {
                continue;
            }
            var device = Service.CreateDevice1(path);
            var candidate = BluetoothAddress.Parse(await device.GetAddressAsync().WaitAsync(ct).ConfigureAwait(false));
            if (candidate == address)
            {
                return path;
            }
        }
        return null;
    }

    private static bool TryMatch(Dictionary<ushort, VariantValue> data, out Switch2ProAdvertisement match)
    {
        foreach (var (companyId, value) in data)
        {
            var unwrapped = value;
            while (unwrapped.Type == VariantValueType.Variant)
            {
                unwrapped = unwrapped.GetVariantValue();
            }
            if (unwrapped.Type == VariantValueType.Array &&
                Switch2ProAdvertisement.TryParse(companyId, unwrapped.GetArray<byte>(), out match))
            {
                return true;
            }
        }
        match = default;
        return false;
    }

    private static async Task<string> GetDeviceNameAsync(Device1 device, Switch2ProAdvertisement match)
    {
        try
        {
            var name = await device.GetNameAsync().ConfigureAwait(false);
            if (!string.IsNullOrWhiteSpace(name))
            {
                return name;
            }
        }
        catch (DBusErrorReplyException)
        {
        }
        return $"Switch 2 Pro Controller ({match.ModeName})";
    }
}
