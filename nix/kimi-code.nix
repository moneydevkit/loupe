# Kimi Code CLI. Moonshot AI's terminal coding agent, distributed as a
# bun-compiled single binary. The bundled JS blob sits past the end of the
# ELF image, so patchelf/strip truncate it (SIGILL at startup). Instead the
# binary is kept byte-identical and exec'd through glibc's ld-linux with
# libstdc++ on the library path.
# See: https://github.com/MoonshotAI/kimi-code
#
# Updating: bump `version`, refresh `hash` with
#   nix store prefetch-file --hash-type sha256 \
#     'https://github.com/MoonshotAI/kimi-code/releases/download/%40moonshot-ai%2Fkimi-code%40<version>/kimi-code-linux-x64.zip'
# then `nix build .#kimi-code` and check the surface loupe-worker drives
# (crates/loupe-worker/src/llm/kimi_cli.rs):
#   result/bin/kimi --version
#   result/bin/kimi --help    # -p, -m, --output-format still present;
#                             # still no --mcp-config flag (MCP config is
#                             # file-based, written into the sandbox HOME)
{
  stdenv,
  lib,
  fetchurl,
  unzip,
  runtimeShell,
}:

stdenv.mkDerivation rec {
  pname = "kimi-code";
  version = "0.34.0";

  src = fetchurl {
    url = "https://github.com/MoonshotAI/kimi-code/releases/download/%40moonshot-ai%2Fkimi-code%40${version}/kimi-code-linux-x64.zip";
    hash = "sha256-iFWH8gpR2U3KGPyb/xEkZULAF/t6xFmpaTu6o8pnsZk=";
  };

  sourceRoot = ".";

  nativeBuildInputs = [ unzip ];

  dontPatchELF = true;
  dontStrip = true;

  installPhase = ''
    runHook preInstall
    install -Dm755 kimi $out/libexec/kimi
    mkdir -p $out/bin
    {
      echo '#!${runtimeShell}'
      echo 'exec ${stdenv.cc.bintools.dynamicLinker} --library-path ${
        lib.makeLibraryPath [ stdenv.cc.cc.lib ]
      } '"$out"'/libexec/kimi "$@"'
    } > $out/bin/kimi
    chmod +x $out/bin/kimi
    runHook postInstall
  '';

  meta = with lib; {
    description = "Kimi Code CLI, Moonshot AI's coding agent for the terminal";
    homepage = "https://github.com/MoonshotAI/kimi-code";
    license = licenses.mit;
    sourceProvenance = with sourceTypes; [ binaryNativeCode ];
    platforms = [ "x86_64-linux" ];
    mainProgram = "kimi";
  };
}
