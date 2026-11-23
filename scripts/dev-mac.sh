#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Dev local en macOS: cierra la app, recompila el .app, lo firma (conserva permisos) y lo abre.
set -e
cd "$(dirname "$0")/.."
pkill -x octopus 2>/dev/null || true
npx --yes @tauri-apps/cli@2 build --bundles app
./scripts/sign.sh
open target/release/bundle/macos/octopus.app
