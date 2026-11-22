#!/usr/bin/env bash
# PipeWire 1.x headers for building the Linux client on Ubuntu 22.04.
#
# The `pipewire` crate needs PipeWire 1.x headers, but the Linux client is
# built on Ubuntu 22.04 (PipeWire 0.3.48) so that it runs on systems from 2022.
# The headers come from Ubuntu 24.04's packages in the official Ubuntu archive
# (checksums pinned below); the client links against Ubuntu 22.04's own
# libpipewire-0.3.so.0 (package libpipewire-0.3-0), so if the client ever needs
# a function that the old library lacks, the build fails instead of the client
# failing to start on users' machines.
#
# Run after installing libpipewire-0.3-0 and libclang-dev; sets PKG_CONFIG_PATH
# for the following steps (through GITHUB_ENV) and prints it.
set -euo pipefail

dir="${RUNNER_TEMP:-/tmp}/pipewire-headers"
base=http://archive.ubuntu.com/ubuntu/pool/main/p/pipewire
mkdir -p "$dir/debs"

# Ubuntu 24.04's release versions: the release pocket never changes, so these
# files stay in the archive for as long as 24.04 is supported.
while read -r sum name; do
    curl -fsSL -o "$dir/debs/$name" "$base/$name"
    echo "$sum  $dir/debs/$name" | sha256sum -c -
    dpkg-deb -x "$dir/debs/$name" "$dir"
done <<'EOF'
c0b34ed6a34391a94c0711966dbea9b6b5a98758d88813824c05d6662e0b60c9 libpipewire-0.3-dev_1.0.5-1_amd64.deb
6c99d162e23e13cacf8bc3edc312c60545281078398e52e34dc5ebb4209649dc libspa-0.2-dev_1.0.5-1_amd64.deb
EOF

lib="$dir/usr/lib/x86_64-linux-gnu"
sed -i "s|^prefix=.*|prefix=$dir/usr|" "$lib"/pkgconfig/*.pc
# Link against this system's library, not the newer one the headers describe.
ln -sf /usr/lib/x86_64-linux-gnu/libpipewire-0.3.so.0 "$lib/libpipewire-0.3.so"

echo "PKG_CONFIG_PATH=$lib/pkgconfig" >> "${GITHUB_ENV:-/dev/null}"
echo "PKG_CONFIG_PATH=$lib/pkgconfig"
