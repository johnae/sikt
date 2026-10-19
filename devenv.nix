{ pkgs, ... }:

{
  languages.rust = {
    enable = true;
    toolchainFile = ./rust-toolchain.toml;
  };

  packages = [
    # Rust workflow
    pkgs.cargo-nextest
    pkgs.cargo-insta
    pkgs.cargo-deny
    pkgs.cargo-llvm-cov
    pkgs.cargo-fuzz
    pkgs.bacon

    # Benchmarks against the tools sikt replaces
    pkgs.ripgrep
    pkgs.hyperfine

    # Luau bindings: the analyzer checks sikt.d.luau
    pkgs.luau
    pkgs.luau-lsp
    pkgs.stylua

    pkgs.jq
  ];
}
