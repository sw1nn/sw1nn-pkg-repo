default:
    @just --list

# Refuse to release from any branch other than main.
_assert-main:
    #!/usr/bin/env bash
    set -euo pipefail
    branch=$(git branch --show-current)
    if [ "$branch" != "main" ]; then
        echo "ERROR: releases can only be run from main (currently on '$branch')" >&2
        exit 1
    fi

# Bump the version, tag, run the release hooks (see cog.toml), then package.
release type='auto': _assert-main && package
    cog bump --{{type}}

# Build the Arch packages in a clean chroot and upload the current version.
package:
    #!/usr/bin/env bash
    set -euo pipefail
    pkgver=$(sed -n 's/^pkgver=//p' packaging/arch/PKGBUILD)
    sw1nn-makepkg-chroot -C packaging/arch
    sw1nn-pkg-ctl upload packaging/arch/*-"$pkgver"-*.pkg.tar.zst
