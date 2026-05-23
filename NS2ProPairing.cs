using System.Security.Cryptography;

namespace Ns2Pro.BleBridge;

internal static class NS2ProPairing
{
    public static async Task PairHostAsync(IControllerCommandChannel controller, ulong hostAddress, CancellationToken ct)
    {
        if (hostAddress == 0)
        {
            throw new ArgumentOutOfRangeException(nameof(hostAddress), "Host Bluetooth address cannot be zero.");
        }

        var secondary = hostAddress - 1;
        var payload = new byte[14];
        payload[0] = 0x00;
        payload[1] = 0x02;
        for (var i = 0; i < 6; i++)
        {
            payload[2 + i] = (byte)(hostAddress >> (8 * i));
            payload[8 + i] = (byte)(secondary >> (8 * i));
        }

        var data = NS2ProProtocol.ResponsePayload(
            await controller.SendCommandAsync(NS2ProProtocol.Command(0x15, 0x01, payload), ct).ConfigureAwait(false),
            0x15,
            0x01);
        if (data.Length < 9 || data[0] != 1)
        {
            throw new InvalidDataException("Address exchange failed.");
        }

        var hostKey = RandomNumberGenerator.GetBytes(16);
        data = NS2ProProtocol.ResponsePayload(
            await controller.SendCommandAsync(NS2ProProtocol.Command(0x15, 0x04, [0, .. hostKey]), ct).ConfigureAwait(false),
            0x15,
            0x04);
        if (data.Length < 17 || data[0] != 1)
        {
            throw new InvalidDataException("Key exchange failed.");
        }

        var ltk = new byte[16];
        for (var i = 0; i < ltk.Length; i++)
        {
            ltk[i] = (byte)(hostKey[i] ^ data[1 + i]);
        }

        var challenge = RandomNumberGenerator.GetBytes(16);
        data = NS2ProProtocol.ResponsePayload(
            await controller.SendCommandAsync(NS2ProProtocol.Command(0x15, 0x02, [0, .. challenge]), ct).ConfigureAwait(false),
            0x15,
            0x02);
        if (data.Length < 17 || data[0] != 1)
        {
            throw new InvalidDataException("LTK confirmation failed.");
        }

        var reversedLtk = new byte[16];
        WriteReversed(ltk, reversedLtk);
        using var aes = Aes.Create();
        aes.Mode = CipherMode.ECB;
        aes.Padding = PaddingMode.None;
        aes.Key = reversedLtk;
        using var encryptor = aes.CreateEncryptor();
        var reversedChallenge = new byte[16];
        WriteReversed(challenge, reversedChallenge);
        var expected = encryptor.TransformFinalBlock(reversedChallenge, 0, reversedChallenge.Length);
        if (!data[1..17].SequenceEqual(expected))
        {
            throw new InvalidDataException("Controller LTK confirmation response did not match.");
        }

        data = NS2ProProtocol.ResponsePayload(
            await controller.SendCommandAsync(NS2ProProtocol.Command(0x15, 0x03, [0]), ct).ConfigureAwait(false),
            0x15,
            0x03);
        if (data.Length == 0 || data[0] != 1)
        {
            throw new InvalidDataException("Pairing finalise failed.");
        }
    }

    private static void WriteReversed(ReadOnlySpan<byte> source, Span<byte> destination)
    {
        for (var i = 0; i < source.Length; i++)
        {
            destination[i] = source[source.Length - 1 - i];
        }
    }
}
