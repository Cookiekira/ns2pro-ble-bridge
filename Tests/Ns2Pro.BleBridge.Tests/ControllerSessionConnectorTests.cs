using System.Security.Cryptography;

namespace Ns2Pro.BleBridge.Tests;

public sealed class ControllerSessionConnectorTests
{
    [Fact]
    public async Task FirstDiscoveryPairsWithBackendAdapterAndSavesCache()
    {
        var cache = TempCachePath();
        var transport = new PairingTransport();
        await using var backend = new FakeBackend(transport)
        {
            ScanResult = new BleDeviceInfo(0x112233445566, "fake controller"),
            AdapterAddress = 0xAABBCCDDEEFF
        };
        var connector = new ControllerSessionConnector(CreateOptions(cache), backend, new Logger(LogLevel.Error));

        await using var controller = await connector.ConnectAsync(TestContext.Current.CancellationToken);

        Assert.Equal(backend.ScanResult.Address, controller.Address);
        Assert.Equal(new byte[] { 0x01, 0x04, 0x02, 0x03 }, transport.PairingSubcommands);
        Assert.Equal(backend.ScanResult.Address, CachedControllerStore.Load(cache));
        Assert.Equal(1, backend.AdapterAddressRequests);
    }

    [Fact]
    public async Task HostAddressOverrideSkipsAdapterLookup()
    {
        var cache = TempCachePath();
        var transport = new PairingTransport();
        await using var backend = new FakeBackend(transport)
        {
            ScanResult = new BleDeviceInfo(0x112233445566, "fake controller")
        };
        var options = CreateOptions(cache) with { HostAddress = 0x010203040506 };
        var connector = new ControllerSessionConnector(options, backend, new Logger(LogLevel.Error));

        await using var controller = await connector.ConnectAsync(TestContext.Current.CancellationToken);

        Assert.Equal(0, backend.AdapterAddressRequests);
        Assert.Equal(new byte[] { 0x01, 0x04, 0x02, 0x03 }, transport.PairingSubcommands);
    }

    [Fact]
    public async Task FailedCachedConnectionDeletesCacheForNextScan()
    {
        var cache = TempCachePath();
        CachedControllerStore.Save(cache, 0x112233445566);
        await using var backend = new FakeBackend(new PairingTransport()) { ConnectFailure = new IOException("offline") };
        var connector = new ControllerSessionConnector(CreateOptions(cache), backend, new Logger(LogLevel.Error));

        await Assert.ThrowsAsync<IOException>(() => connector.ConnectAsync(TestContext.Current.CancellationToken));

        Assert.Null(CachedControllerStore.Load(cache));
    }

    private static CliOptions CreateOptions(string cache) => new(
        UsbAddr: "localhost:3241",
        DeviceAddress: null,
        PairKnownDevice: false,
        HostAddress: null,
        ForgetDevice: false,
        CacheFile: cache,
        NoAutoAttach: true,
        FeatureFlags: NS2ProProtocol.FeatureButtons,
        LogLevel: LogLevel.Error);

    private static string TempCachePath() =>
        Path.Combine(Path.GetTempPath(), "ns2pro-tests", Guid.NewGuid().ToString("N"), "controller-cache.json");

    private sealed class FakeBackend(PairingTransport transport) : IBluetoothBackend
    {
        public BleDeviceInfo ScanResult { get; init; } = new(1, "fake");
        public ulong AdapterAddress { get; init; } = 2;
        public Exception? ConnectFailure { get; init; }
        public int AdapterAddressRequests { get; private set; }

        public Task<BleDeviceInfo> ScanAsync(CancellationToken ct) => Task.FromResult(ScanResult);

        public Task<IBleTransport> ConnectAsync(ulong address, CancellationToken ct) =>
            ConnectFailure is null
                ? Task.FromResult<IBleTransport>(transport)
                : Task.FromException<IBleTransport>(ConnectFailure);

        public Task<ulong> GetAdapterAddressAsync(CancellationToken ct)
        {
            AdapterAddressRequests++;
            return Task.FromResult(AdapterAddress);
        }

        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }

    private sealed class PairingTransport : IBleTransport
    {
        private byte[]? _hostKey;
        public List<byte> PairingSubcommands { get; } = [];
        public Task Disconnected => Task.Delay(Timeout.InfiniteTimeSpan);
        public Task InitializeAsync(CancellationToken ct) => Task.CompletedTask;
        public Task EnableInputReportsAsync(Action<byte[]> handler, CancellationToken ct) => Task.CompletedTask;
        public Task WriteVibrationAsync(byte[] packet, CancellationToken ct) => Task.CompletedTask;
        public ValueTask DisposeAsync() => ValueTask.CompletedTask;

        public Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct)
        {
            var subcommand = command[3];
            if (command[0] != 0x15)
            {
                return Task.FromResult(Response(command[0], subcommand, []));
            }

            PairingSubcommands.Add(subcommand);
            return Task.FromResult(subcommand switch
            {
                0x01 => Response(0x15, subcommand, [1, 0, 0, 0, 0, 0, 0, 0, 0]),
                0x04 => KeyExchange(command),
                0x02 => Confirm(command),
                0x03 => Response(0x15, subcommand, [1]),
                _ => throw new InvalidOperationException($"Unexpected pairing subcommand {subcommand:X2}.")
            });
        }

        private byte[] KeyExchange(byte[] command)
        {
            _hostKey = command.AsSpan(9, 16).ToArray();
            return Response(0x15, 0x04, [1, .. new byte[16]]);
        }

        private byte[] Confirm(byte[] command)
        {
            var key = _hostKey ?? throw new InvalidOperationException("Key exchange was not performed.");
            Array.Reverse(key);
            var challenge = command.AsSpan(9, 16).ToArray();
            Array.Reverse(challenge);
            using var aes = Aes.Create();
            aes.Mode = CipherMode.ECB;
            aes.Padding = PaddingMode.None;
            aes.Key = key;
            var encrypted = aes.EncryptEcb(challenge, PaddingMode.None);
            return Response(0x15, 0x02, [1, .. encrypted]);
        }

        private static byte[] Response(byte command, byte subcommand, byte[] payload)
        {
            var response = new byte[8 + payload.Length];
            response[0] = command;
            response[3] = subcommand;
            payload.CopyTo(response, 8);
            return response;
        }
    }
}
