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
set -euo pipefail

version="${1:?usage: release-notes.sh <version>}"
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

cat <<EOF
${notes}

## Downloads

| File | Use |
| --- | --- |
| \`ContextPassword-${version}-x64-setup.exe\` | Installer. Adds a Start Menu entry and an uninstaller. |
| \`ContextPassword-${version}-x64-portable.exe\` | The application on its own, nothing to install. |
| \`ContextPassword-${version}-x64.msi\` | For deployment through Group Policy or Intune. |

These builds are unsigned, so Windows SmartScreen warns on first run. Choose **More info**, then
**Run anyway**.
EOF
