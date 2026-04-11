{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    llm-agents = {
      url = "github:numtide/llm-agents.nix";

      inputs = {
        nixpkgs.follows = "nixpkgs";
        treefmt-nix.follows = "treefmt-nix";
      };
    };
  };

  outputs =
    {
      nixpkgs,
      llm-agents,
      rust-overlay,
      treefmt-nix,
      ...
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      forEachSupportedSystem =
        f:
        nixpkgs.lib.genAttrs supportedSystems (
          system:
          f (
            let
              pkgs = import nixpkgs {
                inherit system;
                overlays = [ (import rust-overlay) ];
                config.allowUnfree = true;
              };

              rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

              rustPlatform = pkgs.makeRustPlatform {
                rustc = rust;
                cargo = rust;
              };

              formatter =
                (treefmt-nix.lib.evalModule pkgs {
                  projectRootFile = "flake.nix";

                  settings = {
                    allow-missing-formatter = true;
                    verbose = 0;

                    global.excludes = [ "*.lock" ];

                    formatter = {
                      nixfmt.options = [ "--strict" ];
                      rustfmt.package = rust;
                    };
                  };

                  programs = {
                    nixfmt.enable = true;
                    oxfmt.enable = true;
                    rustfmt = {
                      enable = true;
                      package = rust;
                    };
                    taplo.enable = true;
                  };
                }).config.build.wrapper;

              mkPackage =
                {
                  name,
                  cargoPackage ? "lobster",
                  mainProgram ? "lobster",
                  buildFeatures ? [ ],
                  buildInputs ? [ ],
                  nativeBuildInputs ? [ ],
                  extraEnv ? { },
                }:
                rustPlatform.buildRustPackage (
                  {
                    inherit name buildInputs buildFeatures;
                    nativeBuildInputs = nativeBuildInputs;
                    src = ./.;
                    cargoBuildFlags = [
                      "-p"
                      cargoPackage
                    ];
                    cargoTestFlags = [
                      "-p"
                      cargoPackage
                    ];
                    doCheck = false;
                    cargoLock = {
                      lockFile = ./Cargo.lock;
                      outputHashes."pylate-rs-1.0.4" = "sha256-eCLCX7+MGMpUumGq3oLPv3cTepHBmSFdVDVhcpEXiZY=";
                    };
                    RUSTFLAGS = "-C target-cpu=native";

                    meta.mainProgram = mainProgram;
                  }
                  // extraEnv
                );
            in
            {
              inherit
                formatter
                mkPackage
                pkgs
                rust
                system
                ;
            }
          )
        );
    in
    {
      packages = forEachSupportedSystem (
        { mkPackage, pkgs, ... }:
        let
          cudaNativeBuildInputs = with pkgs; [
            cudaPackages.cuda_nvcc
            autoAddDriverRunpath
          ];
          cudaBuildInputs = with pkgs.cudaPackages; [
            cuda_nvcc
            cudatoolkit
            cudnn
          ];
          cudaEnv = {
            CUDA_COMPUTE_CAP = "80";
            CUDA_PATH = "${pkgs.cudaPackages.cudatoolkit}";
          };

          # Pre-fetch NVIDIA CUTLASS for candle-flash-attn (cudaforge).
          # The Nix sandbox blocks network access, so we fetch it here and
          # populate cudaforge's cache directory in preBuild.
          cutlassSrc = pkgs.fetchgit {
            url = "https://github.com/NVIDIA/cutlass.git";
            rev = "7d49e6c7e2f8896c47f586706e67e1fb215529dc";
            hash = "sha256-cSWVzyuDC8EidTAZzHbVz0jUNK4zx5AAwfUV6lUXTXs=";
            leaveDotGit = true;
            fetchSubmodules = false;
          };
        in
        {
          default = mkPackage { name = "lobster"; };

          lobster = mkPackage { name = "lobster"; };

          lobster-cuda = mkPackage {
            name = "lobster-cuda";
            buildFeatures = [ "cuda" ];
            nativeBuildInputs = cudaNativeBuildInputs ++ [ pkgs.git ];
            buildInputs = cudaBuildInputs;
            extraEnv = cudaEnv // {
              CUDAFORGE_HOME = "/tmp/cudaforge-cache";
            };
            extraPreBuild = ''
              mkdir -p $CUDAFORGE_HOME/git/checkouts
              cp -r ${cutlassSrc} $CUDAFORGE_HOME/git/checkouts/cutlass-7d49e6c7e2f8896c
              chmod -R u+w $CUDAFORGE_HOME/git/checkouts/cutlass-7d49e6c7e2f8896c
            '';
          };

          lobster-metal = mkPackage {
            name = "lobster-metal";
            buildFeatures = [ "metal" ];
          };
        }
      );

      formatter = forEachSupportedSystem ({ formatter, ... }: formatter);

      devShells = forEachSupportedSystem (
        {
          pkgs,
          rust,
          formatter,
          ...
        }:
        {
          default = pkgs.mkShell (
            {
              name = "lobster";

              buildInputs =
                with pkgs;
                [
                  llm-agents.packages.${pkgs.stdenv.hostPlatform.system}.beads-rust
                  formatter
                  rust

                  bacon
                  bun
                  cargo-deny
                  cargo-mutants
                  cargo-nextest
                ]
                ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux (
                  with pkgs.cudaPackages;
                  [
                    cuda_nvcc
                    cudatoolkit
                    cudnn
                  ]
                );
            }
            // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              CUDA_COMPUTE_CAP = "80";
              CUDA_PATH = "${pkgs.cudaPackages.cudatoolkit}";

              shellHook = ''
                export LD_LIBRARY_PATH="/run/opengl-driver/lib:$LD_LIBRARY_PATH"
              '';
            }
          );
        }
      );
    };
}
