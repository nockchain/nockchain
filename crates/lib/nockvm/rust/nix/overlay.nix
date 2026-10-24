self: super: {
    libaes_siv = self.callPackage ./libaes_siv.nix {};
    urcrypt = self.callPackage ./urcrypt.nix {};
}
