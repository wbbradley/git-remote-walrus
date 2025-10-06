#!/bin/bash
set -e

# Usage: ./scripts/publish-move.sh [testnet|mainnet]
NETWORK=${1:-testnet}

echo "Publishing Sui Move package to $NETWORK..."
cd move/walrus_remote

if [ "$NETWORK" == "testnet" ]; then
    sui client publish --gas-budget 100000000
elif [ "$NETWORK" == "mainnet" ]; then
    echo "WARNING: Publishing to mainnet!"
    read -p "Are you sure? (yes/no) " -n 3 -r
    echo
    if [[ $REPLY == "yes" ]]; then
        sui client publish --gas-budget 100000000
    else
        echo "Cancelled."
        exit 1
    fi
else
    echo "Unknown network: $NETWORK"
    echo "Usage: $0 [testnet|mainnet]"
    exit 1
fi

echo "Publish successful!"
