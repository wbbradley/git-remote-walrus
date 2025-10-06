#!/bin/bash
set -e

# Usage: ./scripts/create-remote.sh [--shared] [--allow <address>]...

echo "Creating RemoteState object..."

# Build the transaction
ARGS=""
for arg in "$@"; do
    ARGS="$ARGS $arg"
done

# TODO: This script needs the package ID from publish-move.sh
# For now, it's a placeholder
echo "ERROR: Package must be published first. Update this script with package ID."
echo "Usage example:"
echo "  sui client call --package <PACKAGE_ID> --module remote_state --function create_remote --gas-budget 10000000"
exit 1
