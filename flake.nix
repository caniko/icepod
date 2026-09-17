{
  description = "Icepod podcast-first desktop recorder";

  inputs = {
    harbor-rs.url = "github:caniko/harbor-rs";
    nixpkgs.follows = "harbor-rs/nixpkgs";
    rust-overlay.follows = "harbor-rs/rust-overlay";
    crane.follows = "harbor-rs/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    nixpkgs,
    harbor-rs,
    flake-utils,
    rust-overlay,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };
      toolchain = harbor-rs.lib.mkToolchain {inherit pkgs;};
      inherit (toolchain) craneLib;
      cross = harbor-rs.lib.mkCross {inherit pkgs system;};
      src = craneLib.cleanCargoSource ./.;
      nativeBuildInputs = [pkgs.pkg-config];
      buildInputs =
        pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
          pkgs.alsa-lib
          pkgs.libxkbcommon
          pkgs.vulkan-loader
          pkgs.wayland
        ];
      commonArgs = {
        inherit src nativeBuildInputs buildInputs;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      package = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
    in {
      packages.default = package;
      checks = {
        default = package;
        fmt = craneLib.cargoFmt {inherit src;};
        clippy = craneLib.cargoClippy (commonArgs // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--workspace --all-targets --all-features -- --deny warnings";
        });
      };
      devShells = harbor-rs.lib.mkDevShells {
        inherit pkgs cross;
        inherit (toolchain) craneLib;
        pkgConfigDeps = buildInputs;
        packages = with pkgs; [cargo-audit cargo-deny cargo-llvm-cov cargo-nextest pkg-config];
        extraEnv.LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath buildInputs;
      };
    });
}
