# Sourced by release tasks. Shared verbatim by the Rust repos under arca/rs;
# everything project-specific comes from Cargo.toml.
#
# Sets pkg_name, pkg_version, pkg_bin (the first [[bin]]), release_targets,
# and dist_dir. Artifacts are named for a platform, not a Rust triple, e.g.
# darwin-aarch64 or linux-x86_64-musl (static, runs on any Linux libc).

release_targets=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  aarch64-unknown-linux-musl
  x86_64-unknown-linux-musl
)

pkg_metadata=$(cargo metadata --no-deps --format-version 1)
pkg_name=$(jq -r '.packages[0].name' <<<"$pkg_metadata")
pkg_version=$(jq -r '.packages[0].version' <<<"$pkg_metadata")
pkg_bin=$(jq -r '[.packages[0].targets[] | select(.kind | index("bin")) | .name][0]' <<<"$pkg_metadata")
dist_dir="target/dist"

# aarch64-apple-darwin -> darwin-aarch64, x86_64-unknown-linux-musl -> linux-x86_64-musl
platform_for() {
  case "$1" in
    *-apple-darwin) echo "darwin-${1%%-*}" ;;
    *-linux-*) echo "linux-${1%%-*}-${1##*-}" ;;
    *) echo "unsupported target: $1" >&2; return 1 ;;
  esac
}

# The published platform this host runs (Linux gets the static musl build).
host_platform() {
  local os arch
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)
  [ "$arch" = arm64 ] && arch=aarch64
  if [ "$os" = linux ]; then echo "linux-${arch}-musl"; else echo "${os}-${arch}"; fi
}

tarball_for() { echo "${dist_dir}/${pkg_name}-${pkg_version}-$1.tar.gz"; }
