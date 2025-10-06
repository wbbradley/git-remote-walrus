#!/bin/bash
set -e

# Usage: ./scripts/create-remote.sh <package_id> [--shared] [--allow <address>]...
# Example: ./scripts/create-remote.sh 0x1234... --shared --allow 0xabcd...

if [ -z "$1" ]; then
    echo "ERROR: Package ID required as first argument"
    echo "Usage: $0 <package_id> [--shared] [--allow <address>]..."
    echo ""
    echo "Examples:"
    echo "  # Create owned remote:"
    echo "  $0 0x1234..."
    echo ""
    echo "  # Create shared remote with allowlist:"
    echo "  $0 0x1234... --shared --allow 0xabcd... --allow 0xdef..."
    exit 1
fi

PACKAGE_ID=$1
shift

# Parse remaining arguments for --shared and --allow flags
SHARED=false
ALLOWLIST=()

while [ $# -gt 0 ]; do
    case "$1" in
        --shared)
            SHARED=true
            shift
            ;;
        --allow)
            if [ -z "$2" ]; then
                echo "ERROR: --allow requires an address argument"
                exit 1
            fi
            ALLOWLIST+=("$2")
            shift 2
            ;;
        *)
            echo "ERROR: Unknown argument: $1"
            echo "Usage: $0 <package_id> [--shared] [--allow <address>]..."
            exit 1
            ;;
    esac
done

echo "Creating RemoteState object..."
echo "  Package ID: $PACKAGE_ID"
echo "  Shared: $SHARED"
if [ ${#ALLOWLIST[@]} -gt 0 ]; then
    echo "  Allowlist: ${ALLOWLIST[*]}"
fi

# Create the RemoteState object
echo ""
echo "Calling create_remote()..."
OUTPUT=$(sui client call \
    --package "$PACKAGE_ID" \
    --module remote_state \
    --function create_remote \
    --gas-budget 10000000 \
    --json)

# Parse the object ID from the output
OBJECT_ID=$(echo "$OUTPUT" | jq -r '.objectChanges[] | select(.type == "created") | select(.objectType | contains("RemoteState")) | .objectId')

if [ -z "$OBJECT_ID" ]; then
    echo "ERROR: Failed to extract RemoteState object ID from output"
    echo "$OUTPUT"
    exit 1
fi

echo "✓ RemoteState created: $OBJECT_ID"

# If --shared flag is set, call share_with_allowlist
if [ "$SHARED" = true ]; then
    echo ""
    echo "Converting to shared object..."

    # Build allowlist argument
    ALLOWLIST_ARG=""
    for addr in "${ALLOWLIST[@]}"; do
        ALLOWLIST_ARG="$ALLOWLIST_ARG --args $addr"
    done

    # Call share_with_allowlist
    sui client call \
        --package "$PACKAGE_ID" \
        --module remote_state \
        --function share_with_allowlist \
        --args "$OBJECT_ID" \
        $ALLOWLIST_ARG \
        --gas-budget 10000000

    echo "✓ Converted to shared object with allowlist"
fi

echo ""
echo "RemoteState object ID: $OBJECT_ID"
echo ""
echo "To use this remote with git:"
echo "  git remote add storage walrus::$OBJECT_ID"
