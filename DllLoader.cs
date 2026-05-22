using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

namespace Ns2Pro.BleBridge;

internal static class DllLoader
{
    private const string ResourceName = "Ns2Pro.BleBridge.Native.libVIIPER.dll";
    private const string LibraryName = "libVIIPER";

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

        var extracted = ExtractEmbeddedDll(assembly);
        return NativeLibrary.Load(extracted);
    }

    private static string ExtractEmbeddedDll(Assembly assembly)
    {
        using var stream = assembly.GetManifestResourceStream(ResourceName)
            ?? throw new DllNotFoundException(
                $"{ResourceName} is not embedded. Build with /p:ViiperSourceRoot=<path-to-VIIPER> " +
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

        var path = Path.Combine(dir, "libVIIPER.dll");
        if (!File.Exists(path) || new FileInfo(path).Length != bytes.Length)
        {
            File.WriteAllBytes(path, bytes);
        }
        return path;
    }
}
