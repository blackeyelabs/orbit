{
  description = "Orbit - Local-first, encrypted workspace for notes and diagrams";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: f (import nixpkgs { inherit system; }));
    in {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage rec {
          pname = "orbit";
          version = "0.1.0";
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = with pkgs; [
            pkg-config
            makeWrapper
            clang
          ];

          buildInputs = with pkgs; [
            fontconfig
            freetype
            wayland
            libxkbcommon
            vulkan-loader
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
            xorg.libxcb
            libGL
          ];

          postInstall = ''
            install -Dm644 packaging/linux/io.github.blackeyelabs.orbit.desktop \
              $out/share/applications/io.github.blackeyelabs.orbit.desktop
            install -Dm644 packaging/linux/io.github.blackeyelabs.orbit.metainfo.xml \
              $out/share/metainfo/io.github.blackeyelabs.orbit.metainfo.xml

            for size in 16 24 32 48 64 128 256 512; do
              if [ -f "packaging/linux/icons/hicolor/''${size}x''${size}/apps/orbit.png" ]; then
                install -Dm644 "packaging/linux/icons/hicolor/''${size}x''${size}/apps/orbit.png" \
                  "$out/share/icons/hicolor/''${size}x''${size}/apps/orbit.png"
              fi
            done
            if [ -f "packaging/linux/icons/hicolor/scalable/apps/orbit.png" ]; then
              install -Dm644 "packaging/linux/icons/hicolor/scalable/apps/orbit.png" \
                "$out/share/icons/hicolor/scalable/apps/orbit.png"
            fi

            wrapProgram $out/bin/orbit \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (with pkgs; [
                fontconfig
                freetype
                wayland
                libxkbcommon
                vulkan-loader
                xorg.libX11
                xorg.libXcursor
                xorg.libXi
                xorg.libXrandr
                xorg.libxcb
                libGL
              ])}
          '';

          meta = with pkgs.lib; {
            description = "Local-first, encrypted workspace for notes and diagrams";
            homepage = "https://github.com/blackeyelabs/orbit";
            license = licenses.gpl3Only;
            platforms = supportedSystems;
            mainProgram = "orbit";
          };
        };
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.system}.default}/bin/orbit";
        };
      });
    };
}
