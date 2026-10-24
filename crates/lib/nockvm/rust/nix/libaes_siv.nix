{stdenv, fetchFromGitHub, cmake, asciidoc, openssl, libxml2, libxslt, docbook_xml_dtd_45, docbook_xsl}:
stdenv.mkDerivation {
    pname = "libaes_siv";
    version = "1.latest";

    src = fetchFromGitHub {
        owner = "dfoxfranke";
        repo = "libaes_siv";
        rev = "9681279cfaa6e6399bb7ca3afbbc27fc2e19df4b";
        hash = "sha256:1g4wy0m5wpqx7z6nillppkh5zki9fkx9rdw149qcxh7mc5vlszzi";
    };

    buildInputs = [cmake openssl];

    cmakeFlags = [ "DDISABLE_DOCS" ];

}
