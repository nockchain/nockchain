{ stdenv, fetchFromGitHub, autoreconfHook, autoconf-archive, pkg-config, openssl, cryptopp, secp256k1, libaes_siv }:
let rev = "43479c3262a11e20da5f6218f3b0b3d63931ceea";
in stdenv.mkDerivation {
  pname = "urcrypt";
  version = "git-${rev}";
  src = fetchFromGitHub {
    inherit rev;
    owner = "urbit";
    repo = "urcrypt";
    hash = "sha256-GkhqvhDyhsdzjWpR8uqmhdRdhxdpmLGWXtIUZPAbWZs=";
  };

  # preConfigure = ''
  #   ./autogen.sh
  #   '';

  nativeBuildInputs = [autoreconfHook autoconf-archive pkg-config];

  buildInputs = [openssl cryptopp secp256k1 libaes_siv];
}
