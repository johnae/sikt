{
  description = "sikt: search, extract and replace text with readable patterns";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  # Built packages, for main and every release, so they download.
  nixConfig = {
    extra-substituters = [ "https://sikt.cachix.org" ];
    extra-trusted-public-keys = [
      "sikt.cachix.org-1:A5kyTtlEe6m+Vq0iNAqtd6lg9AlYkv+VC72QezmQ5QM="
    ];
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # `nix run github:johnae/sikt -- PATTERN`; on Linux, `.#static` is one
      # statically linked binary that runs on any Linux of the same CPU.
      packages = forSystems (
        pkgs:
        {
          default = pkgs.callPackage ./nix/package.nix { };
        }
        // nixpkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          static = pkgs.pkgsStatic.callPackage ./nix/package.nix { };
        }
      );

      overlays.default = final: _prev: {
        sikt = final.callPackage ./nix/package.nix { };
      };
    };
}
