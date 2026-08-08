#!/usr/bin/env bash
set -euo pipefail

pnpm run build

rm -rf .open-next
mkdir -p .open-next/assets
cp -R out/. .open-next/assets/
cp sites/worker.js .open-next/worker.js

test -f .open-next/assets/index.html
test -f .open-next/worker.js

echo "Static Sites bundle written to .open-next"
