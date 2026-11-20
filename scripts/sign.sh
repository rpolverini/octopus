#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Firma ad-hoc con un requisito que depende solo del identificador, no del hash del binario:
# así macOS conserva el permiso de Accesibilidad entre compilaciones.
# shortcut: cualquier binario con este identificador pasa el requisito, para dev local; con certificado propio/Developer ID, reemplazar.
set -e
APP="${1:-target/release/bundle/macos/octopus.app}"
codesign --force --deep -s - -i dev.octopus.app -r='designated => identifier "dev.octopus.app"' "$APP"
codesign -dr - "$APP"
