{
  description = "Speedport Custom DynDNS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      naersk,
    }:
    let
      forAllSystems = nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          naersk' = pkgs.callPackage naersk { };
        in
        {
          default = naersk'.buildPackage {
            src = ./.;
          };

          docker = pkgs.dockerTools.buildLayeredImage {
            name = "speedport-custom-dyndns";
            tag = "latest";
            contents = [
              self.packages.${system}.default
              pkgs.cacert
            ];
            config = {
              Cmd = [ "${self.packages.${system}.default}/bin/speedport-custom-dyndns" ];
              ExposedPorts = {
                "3000/tcp" = { };
              };
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
            };
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
