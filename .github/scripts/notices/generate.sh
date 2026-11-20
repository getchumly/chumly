#!/usr/bin/env bash
# Writes THIRD-PARTY-NOTICES.txt for a release archive: the licenses of all
# Rust dependencies (cargo-about, config in about.toml) and, for the client,
# of the bundled fonts and icons.
# Usage: .github/scripts/notices/generate.sh <client|server> <output file>
set -euo pipefail

pkg="$1"
out="$2"
root="$(cd "$(dirname "$0")/../../.." && pwd)"
fonts="$root/client/assets/fonts"

{
    echo "Chumly $pkg: licenses of the third-party software it contains."
    echo "Chumly itself is under the GNU AGPL v3.0 (the first license below)."
    echo
} > "$out"

cargo about generate --locked -c "$root/about.toml" -m "$root/$pkg/Cargo.toml" \
    "$root/.github/scripts/notices/about.hbs" >> "$out"

if [[ "$pkg" == client ]]; then
    for f in "Onest:OFL-Onest.txt" "JetBrains Mono:OFL-JetBrainsMono.txt" "Phosphor Icons:LICENSE-Phosphor.txt"; do
        printf '%s\n' "================================================================================" \
            "${f%%:*} (bundled font)" \
            "--------------------------------------------------------------------------------" ""
        cat "$fonts/${f#*:}"
        echo
    done >> "$out"
fi
