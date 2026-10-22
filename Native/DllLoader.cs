using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

namespace Ns2Pro.BleBridge;

internal static class DllLoader
{
    private const string LibraryName = "libVIIPER";
    private static readonly string s_libraryFile = OperatingSystem.IsWindows() ? "libVIIPER.dll" : "libVIIPER.so";
    private static readonly string s_resourceName = $"Ns2Pro.BleBridge.Native.{s_libraryFile}";

    [ModuleInitializer]
    internal static void Initialize()
    {
        NativeLibrary.SetDllImportResolver(typeof(NativeViiper).Assembly, Resolve);
    }

    private static IntPtr Resolve(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (!libraryName.Equals(LibraryName, StringComparison.OrdinalIgnoreCase))
        {
            return IntPtr.Zero;
        }

        var extracted = ExtractEmbeddedLibrary(assembly);
        return NativeLibrary.Load(extracted);
    }

    internal static string VerifyEmbeddedLibrary()
    {
        var assembly = typeof(DllLoader).Assembly;
        using (var stream = assembly.GetManifestResourceStream(s_resourceName)
            ?? throw new DllNotFoundException($"{s_resourceName} is not embedded."))
        {
            Span<byte> magic = stackalloc byte[4];
            if (stream.Read(magic) != magic.Length || !HasExpectedMagic(magic))
            {
                throw new BadImageFormatException($"Embedded {s_libraryFile} is not a valid library for this platform.");
            }
        }

        var path = ExtractEmbeddedLibrary(assembly);
        var handle = NativeLibrary.Load(path);
        NativeLibrary.Free(handle);
        return path;
    }

    private static string ExtractEmbeddedLibrary(Assembly assembly)
    {
        using var stream = assembly.GetManifestResourceStream(s_resourceName)
            ?? throw new DllNotFoundException(
                $"{s_resourceName} is not embedded. Build with /p:ViiperSourceRoot=<path-to-VIIPER> " +
                "or set VIIPER_SOURCE_ROOT.");

        using var sha = SHA256.Create();
        using var memory = new MemoryStream();
        stream.CopyTo(memory);
        var bytes = memory.ToArray();
        var hash = Convert.ToHexString(sha.ComputeHash(bytes))[..16].ToLowerInvariant();
        var dir = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Ns2Pro.BleBridge",
            "native",
            hash);
        Directory.CreateDirectory(dir);

        var path = Path.Combine(dir, s_libraryFile);
        if (!File.Exists(path) || new FileInfo(path).Length != bytes.Length)
        {
            var tempPath = Path.Combine(dir, $"{Guid.NewGuid():N}.tmp");
            try
            {
                File.WriteAllBytes(tempPath, bytes);
                File.Move(tempPath, path, overwrite: true);
            }
            finally
            {
                if (File.Exists(tempPath))
                {
                    File.Delete(tempPath);
                }
            }
        }
        return path;
    }

    private static bool HasExpectedMagic(ReadOnlySpan<byte> magic) =>
        OperatingSystem.IsWindows()
            ? magic[0] == (byte)'M' && magic[1] == (byte)'Z'
            : magic.SequenceEqual(new byte[] { 0x7F, (byte)'E', (byte)'L', (byte)'F' });
}
