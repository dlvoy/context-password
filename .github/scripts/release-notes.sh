#!/usr/bin/env bash
# Extracts one version's section from CHANGELOG.md for the GitHub Release
# body. Kept as a standalone script rather than inline workflow YAML so the
# markdown doesn't have to survive YAML block-scalar indentation rules, and
# so it can be run and previewed locally:
#
#   bash .github/scripts/release-notes.sh 1.2.3
#
# A missing section is a warning, not a failure — a release should never be
# blocked just because nobody wrote a changelog entry yet.
#
# The optional second argument is the combined-artifacts directory (as
# `publish` in release.yml assembles it). When given, the macOS section is
# only emitted if that directory actually contains a `.dmg` — `release-macos`
# is allowed to fail or be skipped without blocking the Windows release (see
# release.yml), so the notes must never advertise a download that isn't
# attached. With no second argument (e.g. a local preview), both sections
# are always emitted.
set -euo pipefail

version="${1:?usage: release-notes.sh <version> [artifacts-dir]}"
artifacts_dir="${2:-}"
changelog="$(dirname "$0")/../../CHANGELOG.md"

notes="$(awk -v ver="[$version]" '
    /^## \[/ {
        if (found) exit
        if (index($0, ver) > 0) { found = 1; next }
        next
    }
    found { print }
' "$changelog")"

if [ -z "${notes//[$'\t\r\n ']/}" ]; then
    echo "::warning::no CHANGELOG.md section found for version ${version}" >&2
    notes="No changelog entry for this version yet."
fi

have_macos=1
if [ -n "$artifacts_dir" ]; then
    have_macos=0
    for f in "$artifacts_dir"/*.dmg; do
        [ -e "$f" ] && have_macos=1
        break
    done
fi

cat <<EOF
${notes}

## Downloads

### Windows

| File | Use |
| --- | --- |
| \`ContextPassword-${version}-x64-setup.exe\` | Installer. Adds a Start Menu entry and an uninstaller. |
| \`ContextPassword-${version}-x64-portable.exe\` | The application on its own, nothing to install. |
| \`ContextPassword-${version}-x64.msi\` | For deployment through Group Policy or Intune. |

This build is unsigned, so Windows SmartScreen warns on first run. Choose **More info**, then
**Run anyway**.
EOF

if [ "$have_macos" -eq 1 ]; then
    cat <<EOF

### macOS

| File | Use |
| --- | --- |
| \`ContextPassword-${version}-universal.dmg\` | Disk image — open it, drag Context Password into Applications. Runs on Apple Silicon and Intel. |

This build is signed ad-hoc, not notarized, so Gatekeeper blocks it on first launch — see
[Opening it the first time](https://github.com/dlvoy/context-password#opening-it-the-first-time)
for the one-time steps to allow it.
EOF
fi
