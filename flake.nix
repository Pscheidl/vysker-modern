{
  description = "Vývojové prostředí obecního webu Vyskeř";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs = { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
    in {
      devShells = eachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [ rust pkg-config openssl postgresql_18 git curl (python3.withPackages (ps: [ ps.psycopg ps.beautifulsoup4 ])) nodejs pnpm cargo-leptos trunk mailpit docker-compose ];
            RUST_BACKTRACE = "1";
            PYTHONTZPATH = "${pkgs.tzdata}/share/zoneinfo";
            shellHook = ''
              export OBEC_DATABAZE="''${OBEC_DATABAZE:-postgresql://vysker:vysker@127.0.0.1:''${OBEC_PG_PORT:-5432}/vysker}"
              if [ -f scripts/postgres.sh ]; then bash scripts/postgres.sh start; fi
            '';
          };
        });
    };
}
