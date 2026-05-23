namespace Ns2Pro.BleBridge;

internal sealed class ControllerSession(BleController controller, ulong address) : IAsyncDisposable
{
    public BleController Controller { get; } = controller;

    public ulong Address { get; } = address;

    public ValueTask DisposeAsync() => Controller.DisposeAsync();
}
