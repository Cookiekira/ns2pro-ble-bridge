namespace Ns2Pro.BleBridge;

internal interface IControllerOutputTarget
{
    void SendRumble(ReadOnlySpan<byte> left, ReadOnlySpan<byte> right);

    Task SetPlayerLedsAsync(byte mask, CancellationToken ct);
}
