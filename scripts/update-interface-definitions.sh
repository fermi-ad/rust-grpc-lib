#!/bin/bash

# ============== Variables ===================

CARGO_PATH="crates/core/Cargo.toml"
CRATE_VERSION_SEARCH_STRING="version = "
CURRENT_VERSION=""
SED_CONFIG=""
UPDATE_TYPE=""
UPDATED_VERSION=""
CHANGE_DESCRIPTION=""

# ============== Initialization ==============

# Fail if any command has nonzero exit
set -e

usage() {
	echo "Usage: $0 [--minor | --major] --description \"Description of interface changes\""
	exit 1
}

flag_validation() {
	if [ ! -z "$UPDATE_TYPE" ]; then 
	    echo "Version update already specified as $UPDATE_TYPE. Only pass either --minor OR --major, not both."
		exit 1
	fi
}

# Parse flags
while [[ $# -gt 0 ]]; do
    case "$1" in
        --minor)
			flag_validation
            UPDATE_TYPE="MINOR"
			echo "Updating interface-definitions and moving to next MINOR version"
			shift
            ;;
        --major)
			flag_validation
            UPDATE_TYPE="MAJOR"
			echo "Updating interface-definitions and moving to next MAJOR version"
			shift
            ;;
        --description)
            if [ $# -lt 2 ]; then
                echo "Error: --description requires a value" >&2
                usage
            fi
            CHANGE_DESCRIPTION="$2"
            shift 2
            ;;
        -h|--help)
			usage
            ;;
		*)
		    echo "Unknown argument: $1"
			usage
			;;
	esac
done

# Check if the type flag was provided
if [ -z "$UPDATE_TYPE" ]; then
    echo "Error: --major or --minor must be specified"
    usage
fi

if [ -z "${CHANGE_DESCRIPTION//[[:space:]]/}" ]; then
    echo "Error: --description must contain a description of the changes" >&2
    usage
fi

# Configure the sed commands (different between Mac and Linux)
case "$(uname -s)" in
    Darwin) SED_CONFIG="'' " ;;
	Linux) SED_CONFIG="" ;;
	*) 
	    echo "Error: Unable to run on $(uname -s) operating system." >&2
		exit 1
		;;
esac

# ================= Core logic =======================

# Make everything relative to the project root. Anchor on this script's location.
cd "$(dirname "$0")/.."

# Calculate next version 
CURRENT_VERSION=$(sed -n "s/^$CRATE_VERSION_SEARCH_STRING\"\(.*\)\"/\1/p" "$CARGO_PATH")

if [ -z "$CURRENT_VERSION" ]; then 
	echo "Error: Failed to find version in $CARGO_PATH. Cannot perform update." >&2
	exit 1
fi

IFS='.' read -r major minor patch <<< "$CURRENT_VERSION"

if [ "$UPDATE_TYPE" = "MAJOR" ]; then
    major=$((major + 1))
	minor=0
	patch=0
elif [ "$UPDATE_TYPE" = "MINOR" ]; then 
	minor=$((minor + 1))
	patch=0
else 
	echo "Error: unknown update type '$UPDATE_TYPE'." >&2
	exit 1
fi

UPDATED_VERSION="${major}.${minor}.${patch}"

echo "Updating from version $CURRENT_VERSION to version $UPDATED_VERSION"

sed -i $SED_CONFIG"s|$CRATE_VERSION_SEARCH_STRING\"$CURRENT_VERSION\"|$CRATE_VERSION_SEARCH_STRING\"$UPDATED_VERSION\"|" "$CARGO_PATH"
cargo update

echo "Pulling the latest version of interface-definitions"

git submodule update --remote --recursive

if [ "$UPDATE_TYPE" = "MAJOR" ]; then
    category="Breaking Changes"
else
    category="Changed"
fi

first_entry=$(grep -n -m 1 '^## \[' CHANGELOG.md | cut -d: -f1)
if [ -z "$first_entry" ]; then
    echo "Error: Cannot find a version heading in CHANGELOG.md" >&2
    exit 1
fi

# Build alongside the changelog so the final rename is atomic, and preserve its permissions.
updated_changelog=$(mktemp ./CHANGELOG.md.XXXXXX)
trap 'rm -f "$updated_changelog"' EXIT
cp -p CHANGELOG.md "$updated_changelog"
{
    head -n "$((first_entry - 1))" CHANGELOG.md
    printf '## [%s] — %s\n\n### %s\n\n- **Updated the pin of interface-definitions.** %s\n\n---\n\n' \
        "$UPDATED_VERSION" "$(date -u +%F)" "$category" "$CHANGE_DESCRIPTION"
    tail -n "+$first_entry" CHANGELOG.md
} > "$updated_changelog"
mv "$updated_changelog" CHANGELOG.md

echo "Update complete"
