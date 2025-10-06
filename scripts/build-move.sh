#!/bin/bash
set -e

echo "Building Sui Move package..."
cd move/walrus_remote
sui move build
echo "Build successful!"
