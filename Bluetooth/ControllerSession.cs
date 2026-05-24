namespace Ns2Pro.BleBridge;

internal sealed class ControllerSession(BleController controller, ulong address) : IAsyncDisposable
{
    public BleController Controller { get; } = controller;

    public ulong Address { get; } = address;

    public Task Disconnected => Controller.Disconnected;

    public Task WaitForDisconnectAsync(CancellationToken ct) =>
        Disconnected.WaitAsync(ct);

    public ValueTask DisposeAsync() => Controller.DisposeAsync();
}
