{ config, pkgs, ... }:

# Note: This file must be symlinked to the proper home-manager config directory.

let
  webring = (builtins.getFlake "path:/home/ring/webring").packages.x86_64-linux.phwebring;
in {
  home.username = "ring";
  home.homeDirectory = "/home/ring";
  home.stateVersion = "24.11";

  home.packages = [ webring ];

  # This will automatically get restarted when rebuilding the home directory
  systemd.user.services.phwebring = {
    Unit = {
      Description = "Purdue Hackers webring";
    };

    Service = {
      ExecStart = pkgs.writeShellScript "webring-startup.sh" ''
        ${pkgs.bubblewrap}/bin/bwrap --bind /home/ring/webring-data /webring --ro-bind /nix/store /nix/store --ro-bind /etc /etc --tmpfs /tmp --unshare-all --share-net --new-session --chdir /webring --uid 256 --gid 512 --die-with-parent ${webring}/bin/ph-webring
      '';
      Restart = "on-failure";
      Type = "exec";
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };

  # Let Home Manager install and manage itself.
  programs.home-manager.enable = true;
}
