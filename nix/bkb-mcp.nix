# Bitcoin Knowledge Base MCP server. loupe's discovery agent auto-attaches
# it when it is on the worker's PATH, giving the scanner bkb_search /
# bkb_lookup_bip / bkb_lookup_bolt / etc. tools for spec and historical
# context on bitcoin/lightning code the worktree alone won't surface.
# See: https://github.com/tnull/bitcoin-knowledge-base
#
# Updating: bump `version`, then refresh both hashes. `hash` is the crate
# tarball; `cargoHash` is the vendored dependency set. Set each to
# lib.fakeHash, build, and copy the value Nix reports.
{
  rustPlatform,
  fetchCrate,
  pkg-config,
  openssl,
  lib,
}:

rustPlatform.buildRustPackage rec {
  pname = "bkb-mcp";
  version = "0.2.1";

  src = fetchCrate {
    inherit pname version;
    hash = "sha256-5rErDnwm4FRAkRkdqW1UI9U6bl6Y45uXh5Y4CYlIhYw=";
  };

  cargoHash = "sha256-OAU8/dw8M6WvHUCWzahwVddq1pC6/aFRiQ5tgSVeX2k=";

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ openssl ];

  # Runtime tool; skip the test suite to keep installs fast.
  doCheck = false;

  meta = with lib; {
    description = "MCP server for the Bitcoin Knowledge Base (bitcoinknowledge.dev)";
    homepage = "https://github.com/tnull/bitcoin-knowledge-base";
    license = licenses.mit;
    mainProgram = "bkb-mcp";
  };
}
