#!/bin/sh
# Installs crowbot for this user: the newest release's file for this machine, checked against the
# release's SHA256SUMS, then `crowbot install` (into ~/.local/bin). Run it as
#   curl -fsSL https://github.com/emlinhax/crowbot-cli/releases/latest/download/install.sh | sh
# CROWBOT_DOWNLOAD_URL fetches the files from elsewhere instead.
# CEILING: the checksums come from the same release; the signature is not checked here, as that
# needs minisign on the machine. The installed crowbot checks it on every update.
set -eu

base=${CROWBOT_DOWNLOAD_URL:-https://github.com/emlinhax/crowbot-cli/releases/latest/download}

say() { printf 'crowbot: %s\n' "$*" >&2; }

os=$(uname -s)
arch=$(uname -m)
# A shell under Rosetta says x86_64 on Apple silicon.
if [ "$os" = Darwin ] && [ "$arch" = x86_64 ] &&
  [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
  arch=arm64
fi
case "$os $arch" in
"Linux x86_64" | "Linux amd64") file=crowbot-x86_64-unknown-linux-musl ;;
"Darwin arm64") file=crowbot-aarch64-apple-darwin ;;
*)
  say "no release runs on $os $arch yet; there are builds for Linux x86_64, macOS on Apple silicon and Windows x64"
  exit 1
  ;;
esac

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    say "downloading needs curl or wget"
    exit 1
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT TERM

say "downloading $file"
fetch "$base/$file" "$tmp/$file"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"

want=$(awk -v f="$file" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
if command -v sha256sum >/dev/null 2>&1; then
  got=$(sha256sum "$tmp/$file" | awk '{ print $1 }')
else
  got=$(shasum -a 256 "$tmp/$file" | awk '{ print $1 }')
fi
if [ -z "$want" ] || [ "$want" != "$got" ]; then
  say "$file does not match SHA256SUMS; nothing was installed"
  exit 1
fi

chmod +x "$tmp/$file"
"$tmp/$file" install
