namespace Ns2Pro.BleBridge.Tests;

public sealed class ControllerOutputTests
{
    [Fact]
    public async Task RoutesLedAndRumbleOutputThroughActiveTransport()
    {
        var transport = new OutputTransport();
        await using var backend = new OutputBackend(transport);
        await using var controller = new BleController(new Logger(LogLevel.Error), NS2ProProtocol.FeatureButtons);
        await controller.ConnectAndInitializeAsync(backend, 1, TestContext.Current.CancellationToken);
        transport.Commands.Clear();

        await controller.SetPlayerLedsAsync(0x09, TestContext.Current.CancellationToken);
        controller.SendRumble(Enumerable.Range(1, 16).Select(i => (byte)i).ToArray(),
            Enumerable.Range(17, 16).Select(i => (byte)i).ToArray());
        var rumble = await transport.RumbleWritten.Task.WaitAsync(TimeSpan.FromSeconds(1), TestContext.Current.CancellationToken);

        Assert.Single(transport.Commands);
        Assert.Equal(NS2ProProtocol.BuildLedCommand(0x09), transport.Commands[0]);
        Assert.Equal(42, rumble.Length);
        Assert.Equal(Enumerable.Range(1, 16).Select(i => (byte)i), rumble.Skip(1).Take(16));
        Assert.Equal(Enumerable.Range(17, 16).Select(i => (byte)i), rumble.Skip(17).Take(16));
    }

    private sealed class OutputBackend(OutputTransport transport) : IBluetoothBackend
    {
        public Task<BleDeviceInfo> ScanAsync(CancellationToken ct) => Task.FromResult(new BleDeviceInfo(1, "fake"));
        public Task<IBleTransport> ConnectAsync(ulong address, CancellationToken ct) => Task.FromResult<IBleTransport>(transport);
        public Task<ulong> GetAdapterAddressAsync(CancellationToken ct) => Task.FromResult(2UL);
        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }

    private sealed class OutputTransport : IBleTransport
    {
        public List<byte[]> Commands { get; } = [];
        public TaskCompletionSource<byte[]> RumbleWritten { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public Task Disconnected => Task.Delay(Timeout.InfiniteTimeSpan);
        public Task InitializeAsync(CancellationToken ct) => Task.CompletedTask;
        public Task EnableInputReportsAsync(Action<byte[]> handler, CancellationToken ct) => Task.CompletedTask;

        public Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct)
        {
            Commands.Add(command);
            return Task.FromResult(new byte[] { command[0], 0, 0, command[3], 0, 0, 0, 0 });
        }

        public Task WriteVibrationAsync(byte[] packet, CancellationToken ct)
        {
            RumbleWritten.TrySetResult(packet);
            return Task.CompletedTask;
        }

        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }
}
