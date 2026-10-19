# The sikt command as a Nix package: `pkgs.callPackage ./nix/package.nix { }`.
# With `pkgsStatic` it's one statically linked binary (musl).
{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "sikt";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../README.md
      ../docs
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  # The release profile with full link-time optimization (Cargo.toml).
  buildType = "dist";
  cargoBuildFlags = [ "--package" "sikt-cli" ];

  # CI runs the tests; here they would only make the build slower.
  doCheck = false;

  meta = {
    description = "Search, extract and replace text with readable patterns";
    homepage = "https://github.com/johnae/sikt";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "sikt";
    platforms = lib.platforms.unix;
  };
}
