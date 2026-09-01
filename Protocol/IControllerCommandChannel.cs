namespace Ns2Pro.BleBridge;

internal interface IControllerCommandChannel
{
    Task<byte[]> SendCommandAsync(byte[] command, CancellationToken ct);
}
