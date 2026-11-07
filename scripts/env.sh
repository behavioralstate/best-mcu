# Sourced by the scripts.
#
# ESP-IDF and its tools in one place for every build, the board's and QEMU's, unless one is set: esp-idf-sys would
# otherwise guess a place from each build's output folder and fetch ESP-IDF once for each.
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="$(cygpath -m "$repo" 2>/dev/null || echo "$repo")"  # cargo on Windows reads C:/..., never /c/...
export ESP_IDF_TOOLS_INSTALL_DIR="${ESP_IDF_TOOLS_INSTALL_DIR:-custom:$repo/firmware/.embuild/espressif}"

# In Git Bash with the MSVC Build Tools environment loaded, coreutils' `link` comes before MSVC's on PATH and the
# host's build scripts fail to link: MSVC's tools are put first.
if [ -n "${VCToolsInstallDir:-}" ] && command -v cygpath >/dev/null; then
  PATH="$(cygpath -u "$VCToolsInstallDir")bin/Hostx64/x64:$PATH"
  export PATH
fi
