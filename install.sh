#!/usr/bin/env bash
#
# ES Runtime installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.sh | bash
#
# Installs both binaries by default — `esrun`, the server runtime, and `esdev`,
# the development toolchain — into ~/.es-runtime/bin, verifying each SHA-256
# checksum when the release ships one.
#
#   --only=esrun          just the server runtime (what a server or CI image wants)
#   --only=esdev          just the development binary
#
#   ESRUN_VERSION=0.24.0  pin one binary; either `0.24.0` or `esrun@0.24.0`
#   ESDEV_VERSION=0.1.0   likewise
#   ES_RUNTIME_INSTALL    install prefix (default ~/.es-runtime)
#
# Releases are tagged per binary — `esrun@0.24.0`, `esdev@0.1.0` — so the
# version for each is resolved from the newest tag carrying *its* prefix.
# Asking GitHub for `/releases/latest` would return whichever binary was
# published most recently, which is how this script used to try to download an
# esrun archive from an esdev release.
set -euo pipefail

REPO="Open-Tech-Foundation/ES-Runtime"
# ESRUN_INSTALL is the pre-0.25 name, still honoured so an existing setup that
# exports it keeps working.
INSTALL_DIR="${ES_RUNTIME_INSTALL:-${ESRUN_INSTALL:-$HOME/.es-runtime}}"
BIN_DIR="$INSTALL_DIR/bin"
LEGACY_BIN_DIR="$HOME/.esrun/bin"

red() { printf '\033[31m%s\033[0m\n' "$*"; }
bold() { printf '\033[1m%s\033[0m\n' "$*"; }
dim() { printf '\033[2m%s\033[0m\n' "$*"; }

err() {
  red "error: $*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || err "curl is required"
command -v tar >/dev/null 2>&1 || err "tar is required"

# --- what to install --------------------------------------------------------
BINS="esrun esdev"
for arg in "$@"; do
  case "$arg" in
    --only=esrun) BINS="esrun" ;;
    --only=esdev) BINS="esdev" ;;
    --only=*) err "unknown --only value: ${arg#--only=} (expected esrun or esdev)" ;;
    -h | --help)
      sed -n '3,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) err "unknown option: $arg" ;;
  esac
done

# --- detect platform --------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux) os_part="linux" ;;
  Darwin) os_part="macos" ;;
  *) err "unsupported OS: $os (use install.ps1 on Windows)" ;;
esac
case "$arch" in
  x86_64 | amd64) arch_part="x86-64" ;;
  arm64 | aarch64) arch_part="arm64" ;;
  *) err "unsupported architecture: $arch" ;;
esac
# Release assets are named `<bin>-<os>-<arch>` by the otf-release tool
# (see release.toml), e.g. `esrun-linux-x86-64.tar.gz`.
target="${os_part}-${arch_part}"

# The newest release tag for one binary, e.g. `esrun@0.24.0`.
#
# Tags are listed newest-first by the releases API, so the first match wins.
# `esrun` also answers to the pre-0.24 `v<version>` tags (release.toml's
# legacy_tag_formats), which is why its pattern accepts both.
latest_tag() {
  bin="$1"
  case "$bin" in
    esrun) pattern='^(esrun@|v[0-9])' ;;
    *) pattern="^${bin}@" ;;
  esac
  curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=100" |
    grep -oE '"tag_name": *"[^"]+"' |
    cut -d'"' -f4 |
    grep -E "$pattern" |
    head -1
}

install_one() {
  bin="$1"
  # ESRUN_VERSION / ESDEV_VERSION.
  var="$(printf '%s' "$bin" | tr '[:lower:]' '[:upper:]')_VERSION"
  eval "pinned=\${$var:-}"

  if [ -n "$pinned" ]; then
    # A bare `0.24.0` is the friendly spelling; a full `esrun@0.24.0` or a
    # legacy `v0.23.0` is passed through as written.
    case "$pinned" in
      *@* | v*) tag="$pinned" ;;
      *) tag="${bin}@${pinned}" ;;
    esac
  else
    tag="$(latest_tag "$bin")"
    [ -n "$tag" ] || err "could not find a released $bin (set $var)"
  fi

  name="${bin}-${target}"
  url="https://github.com/$REPO/releases/download/${tag}/${name}.tar.gz"

  bold "Installing $bin ${tag#*@} (${target})"
  dim "  from $url"

  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064  # $tmp is expanded now on purpose.
  trap "rm -rf '$tmp'" RETURN

  curl -fSL --progress-bar "$url" -o "$tmp/$name.tar.gz" ||
    err "download failed — is there a $bin release asset for $target?"

  # Checksums, when present, live in one `checksums.txt` per release
  # (`<hash>  <archive>` lines); pull out the line for our archive and verify
  # it. A release without a checksums.txt is not fatal — verification is
  # skipped.
  sums_url="https://github.com/$REPO/releases/download/${tag}/checksums.txt"
  if curl -fsSL "$sums_url" -o "$tmp/checksums.txt" 2>/dev/null &&
    grep " ${name}.tar.gz\$" "$tmp/checksums.txt" > "$tmp/$name.tar.gz.sha256"; then
    if command -v shasum >/dev/null 2>&1; then
      (cd "$tmp" && shasum -a 256 -c "$name.tar.gz.sha256" >/dev/null) ||
        err "checksum verification failed"
    elif command -v sha256sum >/dev/null 2>&1; then
      (cd "$tmp" && sha256sum -c "$name.tar.gz.sha256" >/dev/null) ||
        err "checksum verification failed"
    fi
    dim "  checksum verified"
  else
    dim "  no checksums.txt for this release — skipping verification"
  fi

  # The archive holds the binary at its root.
  tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
  mkdir -p "$BIN_DIR"
  install -m 0755 "$tmp/$bin" "$BIN_DIR/$bin"
  dim "  installed to $BIN_DIR/$bin"
}

for bin in $BINS; do
  install_one "$bin"
done

bold ""
installed=""
for bin in $BINS; do installed="$installed $bin"; done
bold "Installed$installed to $BIN_DIR"

# The pre-0.25 location. Left in place — removing binaries this script did not
# put there is not its call — but a stale copy earlier in PATH would shadow what
# was just installed, which is the confusing failure worth naming.
if [ -d "$LEGACY_BIN_DIR" ] && [ "$LEGACY_BIN_DIR" != "$BIN_DIR" ]; then
  echo
  red "note: an older install remains at $LEGACY_BIN_DIR"
  dim "  It is not removed automatically. If it comes first in PATH it will"
  dim "  shadow the binaries above — remove it, or drop its PATH entry:"
  dim "    rm -rf $HOME/.esrun"
fi

# Suggest a PATH entry if it isn't already there.
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    if [ -t 0 ] || [ -c /dev/tty ]; then
      echo
      printf "Would you like to add ES Runtime to your shell profile automatically? [y/N] "
      # 2>/dev/null: /dev/tty can exist as a device node and still not be
      # openable (a container without a controlling terminal), where the read
      # fails and the manual instructions below are the right answer anyway.
      if read -r ans < /dev/tty 2>/dev/null && { [ "$ans" = "y" ] || [ "$ans" = "Y" ]; }; then
        shell_profile=""
        case "${SHELL:-}" in
          */zsh) shell_profile="$HOME/.zshrc" ;;
          */bash) shell_profile="$HOME/.bashrc" ;;
          *)
            if [ -f "$HOME/.bashrc" ]; then shell_profile="$HOME/.bashrc"
            elif [ -f "$HOME/.zshrc" ]; then shell_profile="$HOME/.zshrc"
            elif [ -f "$HOME/.profile" ]; then shell_profile="$HOME/.profile"
            fi
            ;;
        esac
        if [ -n "$shell_profile" ]; then
          echo "" >> "$shell_profile"
          echo "# ES Runtime" >> "$shell_profile"
          echo "export PATH=\"$BIN_DIR:\$PATH\"" >> "$shell_profile"
          echo
          bold "Added PATH to $shell_profile"
          dim "Restart your terminal or run: source $shell_profile"
        else
          echo
          echo "Could not determine shell profile. Please add it manually:"
          bold "  export PATH=\"$BIN_DIR:\$PATH\""
        fi
      else
        echo
        echo "Please add it manually:"
        bold "  export PATH=\"$BIN_DIR:\$PATH\""
      fi
    else
      echo "Please add it manually:"
      bold "  export PATH=\"$BIN_DIR:\$PATH\""
    fi
    ;;
esac
echo
for bin in $BINS; do dim "Run '$bin --version' to verify."; done
