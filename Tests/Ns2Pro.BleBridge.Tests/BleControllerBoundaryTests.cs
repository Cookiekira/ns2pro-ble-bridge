namespace Ns2Pro.BleBridge.Tests;

public sealed class BleControllerBoundaryTests
{
    [Fact]
    public async Task InitializesThroughBackendAndPublishesParsedInput()
    {
        var transport = new FakeTransport();
        await using var backend = new FakeBackend(transport);
        await using var controller = new BleController(new Logger(LogLevel.Error), NS2ProProtocol.FeatureButtons);
        NS2ProInputState? received = null;
        controller.InputReceived += state => received = state;

        await controller.ConnectAndInitializeAsync(backend, 0x112233445566, CancellationToken.None);
        transport.EmitInput(CreateInputReport());

        Assert.True(transport.Initialized);
        Assert.True(transport.InputNotificationsEnabled);
        Assert.Equal(0x112233445566UL, backend.ConnectedAddress);
        Assert.NotNull(received);
        Assert.True((received.Value.Buttons & (uint)NS2ProButtons.A) != 0);
        Assert.Equal((ushort)0x0800, received.Value.LX);
    }

    [Fact]
    public async Task SurfacesBackendDisconnectSignal()
    {
        var transport = new FakeTransport();
        await using var backend = new FakeBackend(transport);
        await using var controller = new BleController(new Logger(LogLevel.Error), NS2ProProtocol.FeatureButtons);
        await controller.ConnectAndInitializeAsync(backend, 1, CancellationToken.None);

        transport.Disconnect();

        await controller.Disconnected.WaitAsync(TimeSpan.FromSeconds(1), TestContext.Current.CancellationToken);
    }

    private static byte[] CreateInputReport()
    {
        var report = new byte[0x3C];
        report[0x04] = 0x08;
        report[0x0A] = 0x00;
        report[0x0B] = 0x08;
        report[0x0C] = 0x80;
        report[0x0D] = 0x00;
        report[0x0E] = 0x08;
        report[0x0F] = 0x80;
        return report;
    }

    private sealed class FakeBackend(FakeTransport transport) : IBluetoothBackend
    {
        public bool SupportsHostPairing => true;
        public ulong ConnectedAddress { get; private set; }

        public Task<BleDeviceInfo> ScanAsync(CancellationToken ct) =>
            Task.FromResult(new BleDeviceInfo(0x112233445566, "fake"));

        public Task<IBleTransport> ConnectAsync(ulong address, CancellationToken ct)
        {
            ConnectedAddress = address;
            return Task.FromResult<IBleTransport>(transport);
        }

        public Task<ulong> GetAdapterAddressAsync(CancellationToken ct) => Task.FromResult(1UL);
        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }

    private sealed class FakeTransport : IBleTransport
    {
        private readonly TaskCompletionSource _disconnected = new(TaskCreationOptions.RunContinuationsAsynchronously);
        private Action<byte[]>? _input;

        public bool Initialized { get; private set; }
        public bool InputNotificationsEnabled { get; private set; }
        public Task Disconnected => _disconnected.Task;

        public Task InitializeAsync(CancellationToken ct)
        {
            Initialized = true;
            return Task.CompletedTask;
        }

        public Task EnableInputReportsAsync(Action<byte[]> handler, CancellationToken ct)
        {
            _input = handler;
            InputNotificationsEnabled = true;
            return Task.CompletedTask;
        }

        public Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct) =>
            Task.FromResult(new byte[] { command[0], 0, 0, command[3], 0, 0, 0, 0 });

        public Task WriteVibrationAsync(byte[] packet, CancellationToken ct) => Task.CompletedTask;
        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
        public void EmitInput(byte[] report) => _input?.Invoke(report);
        public void Disconnect() => _disconnected.TrySetResult();
    }
}
