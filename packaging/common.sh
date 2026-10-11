# Shared by build_macos.sh / build_windows.sh / build_linux.sh (sourced from the repository root).
#   parse_pack_args "$@"   sets WITH_DATA (0 = the public build: code only; 1 = --with-data: the player's
#                          game data inside, for your own machines only) and SUFFIX for the archive name
#   stage_legal <dir> <target triple>
#                          LICENSE, THIRD-PARTY-LICENSES.txt and the README's disclaimer files
#   stage_game_dir <dir>   the public build's empty game/ with the instructions where the data goes
. packaging/brand.sh
parse_pack_args() {
  WITH_DATA=0
  for a in "$@"; do
    case "$a" in
      --with-data) WITH_DATA=1 ;;
      *) echo "unknown option $a" >&2; exit 2 ;;
    esac
  done
  if [ "$WITH_DATA" = 1 ]; then
    SUFFIX=-with-data
    echo "NOTE: --with-data puts the original game's data (EA's) into the archive: for your own machines only, never publish it." >&2
  else
    SUFFIX=
  fi
}
stage_legal() {
  cp LICENSE "$1/LICENSE.txt"
  python3 packaging/third_party_licenses.py "$1/THIRD-PARTY-LICENSES.txt" --target "$2" >/dev/null
}
stage_game_dir() {
  mkdir -p "$1"
  brand_readme packaging/GAME-DATA.txt > "$1/PUT-GAME-DATA-HERE.txt"
}
